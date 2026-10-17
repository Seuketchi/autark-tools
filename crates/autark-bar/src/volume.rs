//! Volume: the default output's level and mute, from PipeWire on a thread of its own, since
//! PipeWire runs its own loop. Clicking opens Sound: a slider, mute and the output to use.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::rc::Rc;
use std::time::Duration;

use async_channel::Sender;
use gtk::cairo::{Context, LineCap, LineJoin};
use gtk::{gio, glib};
use gtk::prelude::*;
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::node::{Node, NodeListener};
use pw::spa::param::ParamType;
use pw::spa::pod::deserialize::PodDeserializer;
use pw::spa::pod::{Pod, Value, ValueArray};
use pw::types::ObjectType;

use crate::menu;

#[derive(Clone, Copy, Default, PartialEq)]
struct Level {
    /// As shown to people: the cube root of PipeWire's linear volume, the scale `wpctl` uses.
    volume: f32,
    muted: bool,
}

#[derive(Clone, PartialEq)]
struct Sink {
    id: u32,
    description: String,
}

/// What the PipeWire thread reports: the outputs, which is the default, and its level.
#[derive(Clone, Default, PartialEq)]
struct Snapshot {
    sinks: Vec<Sink>,
    default: Option<u32>,
    level: Option<Level>,
}

/// Changes go through `wpctl`: WirePlumber owns the hardware volume, so a volume set directly
/// on the PipeWire node doesn't stick.
fn wpctl(args: &[&str]) {
    let argv: Vec<&OsStr> = std::iter::once(OsStr::new("wpctl")).chain(args.iter().map(OsStr::new)).collect();
    if let Err(err) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
        eprintln!("volume: wpctl: {err}");
    }
}

pub fn new() -> gtk::MenuButton {
    let level = Rc::new(Cell::new(Level::default()));
    let default = Rc::new(Cell::new(None::<u32>));

    let icon = speaker(&level);
    let mute = gtk::Button::builder().child(&speaker(&level)).tooltip_text("Mute").build();
    mute.add_css_class("row");
    mute.connect_clicked({
        let default = default.clone();
        move |_| {
            if let Some(id) = default.get() {
                wpctl(&["set-mute", &id.to_string(), "toggle"]);
            }
        }
    });

    let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    slider.set_draw_value(false);
    slider.set_hexpand(true);
    // Set while the slider is moved to match PipeWire, so that isn't sent back as a change.
    let syncing = Rc::new(Cell::new(false));
    // Dragging sends a value every few milliseconds; only the latest is applied, at most every 60 ms.
    let pending = Rc::new(Cell::new(None::<f64>));
    slider.connect_value_changed({
        let (syncing, pending, default) = (syncing.clone(), pending.clone(), default.clone());
        move |slider| {
            if syncing.get() || pending.replace(Some(slider.value())).is_some() {
                return;
            }
            let (pending, default) = (pending.clone(), default.clone());
            glib::timeout_add_local_once(Duration::from_millis(60), move || {
                if let (Some(value), Some(id)) = (pending.take(), default.get()) {
                    wpctl(&["set-volume", &id.to_string(), &format!("{:.2}", value / 100.0)]);
                }
            });
        }
    });

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.append(&mute);
    controls.append(&slider);
    let outputs = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
    panel.append(&menu::heading("Sound"));
    panel.append(&controls);
    panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    panel.append(&menu::caption("Output"));
    panel.append(&outputs);

    let button = menu::button(&icon, &panel, &["icon"]);
    button.set_visible(false);

    let (updates, received) = async_channel::unbounded();
    std::thread::spawn(move || {
        if let Err(err) = watch(updates) {
            eprintln!("volume: {err}");
        }
    });
    glib::spawn_future_local({
        let button = button.clone();
        async move {
            let mut shown = Snapshot::default();
            while let Ok(update) = received.recv().await {
                let Some(current) = update.level else {
                    button.set_visible(false);
                    continue;
                };
                button.set_visible(true);
                button.set_tooltip_text(Some(&if current.muted {
                    "Muted".to_owned()
                } else {
                    format!("Volume {:.0}%", current.volume * 100.0)
                }));
                level.set(current);
                default.set(update.default);
                if pending.get().is_none() {
                    syncing.set(true);
                    slider.set_value(f64::from(current.volume) * 100.0);
                    syncing.set(false);
                }
                mute.set_tooltip_text(Some(if current.muted { "Unmute" } else { "Mute" }));
                icon.queue_draw();
                if let Some(speaker) = mute.child() {
                    speaker.queue_draw();
                }
                if (&update.sinks, update.default) != (&shown.sinks, shown.default) {
                    while let Some(child) = outputs.first_child() {
                        outputs.remove(&child);
                    }
                    for sink in &update.sinks {
                        let row = menu::row(&sink.description, Some(sink.id) == update.default);
                        let id = sink.id.to_string();
                        row.connect_clicked(move |_| wpctl(&["set-default", &id]));
                        outputs.append(&row);
                    }
                }
                shown = update;
            }
            button.set_visible(false);
        }
    });
    button
}

fn speaker(level: &Rc<Cell<Level>>) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .valign(gtk::Align::Center)
        .build();
    let level = level.clone();
    icon.set_draw_func(move |area, cr, width, height| draw(area, cr, width.min(height), level.get()));
    icon
}

struct SinkState {
    name: String,
    description: String,
    level: Option<Level>,
}

#[derive(Default)]
struct State {
    /// Every audio output; there are only a few, so all are watched.
    sinks: BTreeMap<u32, SinkState>,
    default_sink: Option<String>,
    shown: Option<Snapshot>,
    updates: Option<Sender<Snapshot>>,
}

impl State {
    fn send(&mut self) {
        let default = self
            .sinks
            .iter()
            .find(|(_, sink)| Some(&sink.name) == self.default_sink.as_ref())
            .map(|(id, _)| *id);
        let snapshot = Snapshot {
            sinks: self
                .sinks
                .iter()
                .map(|(id, sink)| Sink { id: *id, description: sink.description.clone() })
                .collect(),
            default,
            level: default.and_then(|id| self.sinks[&id].level),
        };
        if self.shown.as_ref() != Some(&snapshot) {
            if let Some(updates) = &self.updates {
                let _ = updates.send_blocking(snapshot.clone());
            }
            self.shown = Some(snapshot);
        }
    }
}

fn watch(updates: Sender<Snapshot>) -> Result<(), pw::Error> {
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
                        // The short name ("Speaker", "HDMI 1"); descriptions repeat the sound card's long name.
                        let description = props
                            .and_then(|p| p.get("node.nick").or_else(|| p.get("node.description")))
                            .unwrap_or(name);
                        let Ok(node) = registry.bind::<Node, _>(global) else {
                            return;
                        };
                        let id = global.id;
                        let sink = SinkState { name: name.to_owned(), description: description.to_owned(), level: None };
                        state.borrow_mut().sinks.insert(id, sink);
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
                                        sink.level = Some(level);
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
