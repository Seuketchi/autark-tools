//! Calendar: every account's events in one place, over CalDAV. Accounts are signed in once, by
//! `autark-accounts`. While it runs it also serves `org.autark.Calendar1` for Kith.

mod backend;
mod editor;
mod logic;
mod service;
mod window;

use std::sync::Arc;

use gtk::glib;
use gtk::prelude::*;
use serde::Deserialize;

/// `calendar.toml`.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// `monday` (the default), `sunday` or `saturday`.
    pub week_start: String,
}

fn main() -> glib::ExitCode {
    let backend = Arc::new(backend::Backend::default());
    let app = gtk::Application::builder().application_id("org.autark.Calendar").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("calendar.css")));
    let served = backend.clone();
    app.connect_startup(move |_| {
        // The service lives as long as the process; a name already taken (a second copy) isn't an error.
        let service = service::Service { backend: served.clone() };
        match async_io::block_on(zbus::connection::Builder::session().name("org.autark.Calendar1").serve_at(service::PATH, service).build()) {
            Ok(connection) => std::mem::forget(connection),
            Err(err) => eprintln!("calendar: serving org.autark.Calendar1: {err}"),
        }
    });
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        window::Calendar::new(app, backend.clone()).window.present();
    });
    app.run()
}
