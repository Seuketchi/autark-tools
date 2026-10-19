//! The recording item: a red dot and "Recording" while `autark-shot record` is running, and a click stops it.
//! The recorder keeps a pid file in the runtime folder, which is all this reads; it looks once a second, as nothing
//! else tells the bar a recording began.

use std::path::PathBuf;

use gtk::glib;
use gtk::prelude::*;

use crate::shared::Shared;

fn folder() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("autark")
}

/// Whether a recording is going on: the process in the pid file is still an `autark-shot`.
fn running() -> bool {
    let Some(pid) = std::fs::read_to_string(folder().join("record.pid")).ok().and_then(|text| text.trim().parse::<u32>().ok()) else { return false };
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|name| name.trim() == "autark-shot")
}

pub fn source() -> Shared<bool> {
    let shared = Shared::new(running());
    glib::timeout_add_seconds_local(1, {
        let shared = shared.clone();
        move || {
            let now = running();
            if now != shared.get() {
                shared.set(now);
            }
            glib::ControlFlow::Continue
        }
    });
    shared
}

pub fn view(source: &Shared<bool>) -> gtk::Button {
    let dot = gtk::DrawingArea::builder().content_width(10).content_height(10).valign(gtk::Align::Center).build();
    dot.add_css_class("recording-dot");
    dot.set_draw_func(|area, cr, width, height| {
        cr.set_source_color(&area.color());
        cr.arc(f64::from(width) / 2.0, f64::from(height) / 2.0, f64::from(width.min(height)) / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    let label = gtk::Label::new(Some("Recording"));
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&dot);
    content.append(&label);
    let button = gtk::Button::builder().child(&content).tooltip_text("Stop recording").build();
    button.add_css_class("item");
    button.add_css_class("recording");
    button.connect_clicked(|_| {
        let stop = std::fs::create_dir_all(folder()).and_then(|()| std::fs::write(folder().join("record.stop"), b""));
        if let Err(err) = stop {
            eprintln!("recording: stopping it: {err}");
        }
    });
    source.watch(&button, |button, recording| button.set_visible(*recording));
    button
}
