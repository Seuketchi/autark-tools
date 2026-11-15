//! Butler: a desktop window onto the owner's Alfred: what waits for a yes, what Alfred remembers, today's lists, the
//! turn log and what's linked. It holds no data of its own; everything comes over `door` from `alfred window` on the
//! server, and `window` shows it.

mod askpass;
mod door;
mod window;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    if std::env::var_os(door::ASKPASS).is_some() {
        return askpass::run(std::env::args().nth(1).unwrap_or_default());
    }
    let app = gtk::Application::builder().application_id("org.autark.Butler").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("butler.css")));
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        window::Butler::new(app).window.present();
    });
    app.run()
}
