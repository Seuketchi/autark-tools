//! Viewer: pictures in a window of Autark's own. `autark-viewer [file]…` opens each in a window; without one, the
//! window asks for a file.

mod view;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    // Each run has its window; `HANDLES_OPEN` gives it the files from the command line.
    let app = gtk::Application::builder().application_id("org.autark.Viewer").flags(gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("viewer.css")));
    app.connect_activate(|app| view::Viewer::new(app).window.present());
    app.connect_open(|app, files, _| {
        for file in files {
            let viewer = view::Viewer::new(app);
            match file.path() {
                Some(path) => viewer.open(&path),
                None => viewer.fail("Only files on this computer can be opened."),
            }
            viewer.window.present();
        }
    });
    app.run()
}
