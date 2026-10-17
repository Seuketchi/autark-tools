//! Volume: the default output's level and mute, from PipeWire on a thread of its own, since
//! PipeWire runs its own loop. Hidden when PipeWire isn't running.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use async_channel::Sender;
use gtk::cairo::{Context, LineCap, LineJoin};
use gtk::glib;
use gtk::prelude::*;
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::node::{Node, NodeListener};
use pw::spa::param::ParamType;
use pw::spa::pod::deserialize::PodDeserializer;
use pw::spa::pod::{Pod, Value, ValueArray};
use pw::types::ObjectType;

#[derive(Clone, Copy, Default, PartialEq)]
struct Level {
    /// As shown to people: the cube root of PipeWire's linear volume, as `wpctl` shows it.
    volume: f32,
    muted: bool,
}

pub fn new() -> gtk::Box {
    let level = Rc::new(RefCell::new(Level::default()));

    let icon = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .valign(gtk::Align::Center)
        .build();
    icon.set_draw_func({
        let level = level.clone();
        move |area, cr, width, height| draw(area, cr, width.min(height), *level.borrow())
    });

    let item = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    item.add_css_class("item");
    item.add_css_class("icon");
    item.set_visible(false);
    item.append(&icon);

    let (updates, received) = async_channel::unbounded();
    std::thread::spawn(move || {
        if let Err(err) = watch(updates) {
            eprintln!("volume: {err}");
        }
    });
    glib::spawn_future_local({
        let item = item.clone();
        async move {
            while let Ok(update) = received.recv().await {
                let Some(update) = update else {
                    item.set_visible(false);
                    continue;
                };
                item.set_visible(true);
                item.set_tooltip_text(Some(&if update.muted {
                    "Muted".to_owned()
                } else {
                    format!("Volume {:.0}%", update.volume * 100.0)
                }));
                *level.borrow_mut() = update;
                icon.queue_draw();
            }
            item.set_visible(false);
        }
    });
    item
}

#[derive(Default)]
struct State {
    /// Every audio output's `node.name` and level; there are only a few, so all are watched.
    sinks: HashMap<u32, (String, Option<Level>)>,
    default_sink: Option<String>,
    shown: Option<Option<Level>>,
    updates: Option<Sender<Option<Level>>>,
}

impl State {
    fn send(&mut self) {
        let current = self.sinks.values().find_map(|(name, level)| {
            (Some(name) == self.default_sink.as_ref()).then_some(*level).flatten()
        });
        if self.shown != Some(current) {
            self.shown = Some(current);
            if let Some(updates) = &self.updates {
                let _ = updates.send_blocking(current);
            }
        }
    }
}

fn watch(updates: Sender<Option<Level>>) -> Result<(), pw::Error> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let registry_weak = registry.downgrade();

    let state = Rc::new(RefCell::new(State { updates: Some(updates), ..State::default() }));
    // Proxies stop reporting once dropped, so they live here with their listeners.
    let nodes: Rc<RefCell<HashMap<u32, (Node, NodeListener)>>> = Rc::default();
    let metadata: Rc<RefCell<Option<(Metadata, MetadataListener)>>> = Rc::default();

    let _registry_listener = registry
        .add_listener_local()
        .global({
            let state = state.clone();
            let nodes = nodes.clone();
            move |global| {
                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };
                let props = global.props;
                match global.type_ {
                    ObjectType::Node if props.and_then(|p| p.get("media.class")) == Some("Audio/Sink") => {
                        let Some(name) = props.and_then(|p| p.get("node.name")) else {
                            return;
                        };
                        let Ok(node) = registry.bind::<Node, _>(global) else {
                            return;
                        };
                        let id = global.id;
                        state.borrow_mut().sinks.insert(id, (name.to_owned(), None));
                        let listener = node
                            .add_listener_local()
                            .param({
                                let state = state.clone();
                                move |_, _, _, _, param| {
                                    let Some(level) = param.and_then(parse_props) else {
                                        return;
                                    };
                                    let mut state = state.borrow_mut();
                                    if let Some(sink) = state.sinks.get_mut(&id) {
                                        sink.1 = Some(level);
                                    }
                                    state.send();
                                }
                            })
                            .register();
                        node.subscribe_params(&[ParamType::Props]);
                        nodes.borrow_mut().insert(id, (node, listener));
                    }
                    ObjectType::Metadata if props.and_then(|p| p.get("metadata.name")) == Some("default") => {
                        let Ok(bound) = registry.bind::<Metadata, _>(global) else {
                            return;
                        };
                        let listener = bound
                            .add_listener_local()
                            .property({
                                let state = state.clone();
                                move |_, key, _, value| {
                                    if key == Some("default.audio.sink") {
                                        let mut state = state.borrow_mut();
                                        state.default_sink = value.and_then(sink_name);
                                        state.send();
                                    }
                                    0
                                }
                            })
                            .register();
                        *metadata.borrow_mut() = Some((bound, listener));
                    }
                    _ => {}
                }
            }
        })
        .global_remove(move |id| {
            nodes.borrow_mut().remove(&id);
            let mut state = state.borrow_mut();
            if state.sinks.remove(&id).is_some() {
                state.send();
            }
        })
        .register();

    main_loop.run();
    Ok(())
}

/// The volume and mute from a node's `Props`; channels can differ, so the loudest one counts.
fn parse_props(param: &Pod) -> Option<Level> {
    let (_, Value::Object(object)) = PodDeserializer::deserialize_any_from(param.as_bytes()).ok()? else {
        return None;
    };
    let mut level = Level::default();
    let mut found = false;
    for property in object.properties {
        match (property.key, property.value) {
            (pw::spa::sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(volumes))) => {
                level.volume = volumes.into_iter().fold(0.0, f32::max).cbrt();
                found = true;
            }
            (pw::spa::sys::SPA_PROP_mute, Value::Bool(muted)) => level.muted = muted,
            _ => {}
        }
    }
    found.then_some(level)
}

/// `default.audio.sink` holds JSON such as `{ "name": "alsa_output.pci-0000_00_1f.3.analog-stereo" }`.
fn sink_name(value: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(value).ok()?;
    Some(json.get("name")?.as_str()?.to_owned())
}

/// The design's speaker with up to two waves by level, or a cross when muted.
fn draw(area: &gtk::DrawingArea, cr: &Context, size: i32, level: Level) {
    let lit = area.color();
    let dim = lit.with_alpha(lit.alpha() * 0.25);
    let scale = f64::from(size) / 24.0;
    cr.scale(scale, scale);
    cr.set_line_width(2.0);
    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);
    cr.set_source_color(&lit);

    cr.move_to(4.0, 9.0);
    cr.line_to(8.0, 9.0);
    cr.line_to(13.0, 5.0);
    cr.line_to(13.0, 19.0);
    cr.line_to(8.0, 15.0);
    cr.line_to(4.0, 15.0);
    cr.close_path();
    let _ = cr.stroke();

    if level.muted {
        cr.move_to(16.5, 9.5);
        cr.line_to(21.5, 14.5);
        cr.move_to(21.5, 9.5);
        cr.line_to(16.5, 14.5);
        let _ = cr.stroke();
        return;
    }
    // Each wave spans x at height 12 ± half, on a circle of the given radius.
    let lit_waves = match level.volume {
        v if v <= 0.0 => 0,
        v if v <= 0.5 => 1,
        _ => 2,
    };
    for (index, (x, half, radius)) in [(16.5_f64, 3.5_f64, 5.0_f64), (19.0, 6.0, 8.5)].into_iter().enumerate() {
        let centre = x - (radius * radius - half * half).sqrt();
        let end = half.atan2(x - centre);
        cr.new_sub_path();
        cr.arc(centre, 12.0, radius, -end, end);
        cr.set_source_color(if index < lit_waves { &lit } else { &dim });
        let _ = cr.stroke();
    }
}
