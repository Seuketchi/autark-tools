//! Player: video and audio through GStreamer, in a window of Autark's own. `autark-player [file or address]…`
//! opens each in a window; without one, the window asks for a file.

mod media;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    // Each run has its window; `HANDLES_OPEN` gives it the files from the command line.
    let app = gtk::Application::builder().application_id("org.autark.Player").flags(gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("player.css")));
    app.connect_activate(|app| window::Player::new(app).window.present());
    app.connect_open(|app, files, _| {
        for file in files {
            let player = window::Player::new(app);
            player.open(&file.uri());
            player.window.present();
        }
    });
    app.run()
}
