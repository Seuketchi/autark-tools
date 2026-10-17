//! Volume: the default output's level and mute, from PipeWire through `autark_common::audio`, which watches
//! it on a thread of its own. Clicking opens Sound: a slider, mute and the output to use.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

pub use autark_common::audio::Snapshot;
use autark_common::audio::{self, Level, wpctl};
use gtk::cairo::{Context, LineCap, LineJoin};
use gtk::glib;
use gtk::prelude::*;

use crate::menu;
use crate::shared::Shared;

/// Watches PipeWire once for every bar.
pub fn source() -> Shared<Snapshot> {
    let shared = Shared::new(Snapshot::default());
    let received = audio::spawn();
    glib::spawn_future_local({
        let shared = shared.clone();
        async move {
            while let Ok(update) = received.recv().await {
                shared.set(update);
            }
            shared.set(Snapshot::default());
        }
    });
    shared
}

pub fn view(source: &Shared<Snapshot>) -> gtk::MenuButton {
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
    let shown = RefCell::new(Snapshot::default());
    source.watch(&button, move |button, update| {
        let Some(current) = update.output().and_then(|output| output.level) else {
            button.set_visible(false);
            return;
        };
        button.set_visible(true);
        button.set_tooltip_text(Some(&if current.muted { "Muted".to_owned() } else { format!("Volume {:.0}%", current.volume * 100.0) }));
        level.set(current);
        default.set(update.default_output);
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
        let mut shown = shown.borrow_mut();
        let names = |snapshot: &Snapshot| snapshot.outputs.iter().map(|output| (output.id, output.nick.clone())).collect::<Vec<_>>();
        if (names(update), update.default_output) != (names(&shown), shown.default_output) {
            while let Some(child) = outputs.first_child() {
                outputs.remove(&child);
            }
            for output in &update.outputs {
                let row = menu::row(&output.nick, Some(output.id) == update.default_output);
                let id = output.id.to_string();
                row.connect_clicked(move |_| wpctl(&["set-default", &id]));
                outputs.append(&row);
            }
        }
        *shown = update.clone();
    });
    button
}

fn speaker(level: &Rc<Cell<Level>>) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::builder().content_width(16).content_height(16).valign(gtk::Align::Center).build();
    let level = level.clone();
    icon.set_draw_func(move |area, cr, width, height| draw(area, cr, width.min(height), level.get()));
    icon
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
