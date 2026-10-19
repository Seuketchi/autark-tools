//! Calculator: type a sum or press the keys, and the answer shows as you go. Enter (or =) keeps it and starts
//! from it; Esc clears. See `calc` for what it understands.

mod calc;
mod window;

use gtk::prelude::*;
use gtk::glib;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Calc").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("calc.css")));
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        window::Calc::new(app).window.present();
    });
    app.run()
}
