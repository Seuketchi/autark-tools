//! Kith's team: a roster of named agents on the left, the conversation with the one you pick on the right. Each
//! agent has a job and a soul and a conversation of its own, kept by `autark-ai`; this window only shows them and
//! talks to it over D-Bus. See `logic` for what it works out and `window` for how it looks.

mod ai;
mod logic;
mod window;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Kith").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("kith.css")));
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        window::Kith::new(app).window.present();
    });
    app.run()
}
