//! Keyboard shortcuts: every key combination Wayfire answers, named for people and read from its own config, so it
//! is never out of date. Run it again, or press Esc, and it goes.

mod shortcuts;
mod window;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Keys").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("keys.css")));
    app.connect_activate(|app| {
        // Run again while it's open, it closes: the same keys open and close it.
        if let Some(open) = app.active_window() {
            open.close();
            return;
        }
        window::build(app).present();
    });
    app.run()
}
