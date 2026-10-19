//! Clock: alarms, a timer and a stopwatch. The window sets them up; systemd's user timers ring them, so they go off
//! with the window shut. `autark-clock ring <label> [alarm|timer]` is what those timers run: it shows a notification
//! and plays the alert a few times.

mod schedule;
mod state;
mod window;

use std::collections::HashMap;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ring") {
        ring(args.get(1).map_or("Alarm", String::as_str), args.get(2).map_or("alarm", String::as_str));
        return glib::ExitCode::SUCCESS;
    }
    let app = gtk::Application::builder().application_id("org.autark.Clock").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("clock.css")));
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        window::Clock::new(app).window.present();
    });
    app.run()
}

/// Says it's time, loud enough to notice: a notification that stays, and the alert four times.
fn ring(label: &str, kind: &str) {
    let body = if kind == "timer" { "Time’s up" } else { "Alarm" };
    if let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        let hints = HashMap::<&str, glib::Variant>::from([("urgency", 2u8.to_variant())]);
        let args = ("Clock", 0u32, "autark-clock", label, body, Vec::<&str>::new(), hints, -1i32).to_variant();
        let sent = bus.call_sync(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        );
        if let Err(error) = sent {
            eprintln!("clock: notification: {error}");
        }
    }
    for _ in 0..4 {
        autark_common::audio::play(autark_common::audio::Effect::Alert);
        std::thread::sleep(Duration::from_millis(1500));
    }
}
