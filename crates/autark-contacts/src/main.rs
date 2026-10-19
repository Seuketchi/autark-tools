//! Contacts: every account's address books in one place, over CardDAV. Accounts are signed in once, by
//! `autark-accounts`. While it runs it also serves `org.autark.Contacts1` for Kith and for Mail.

mod backend;
mod editor;
mod logic;
mod service;
mod window;

use std::sync::Arc;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let backend = Arc::new(backend::Backend::default());
    let app = gtk::Application::builder().application_id("org.autark.Contacts").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("contacts.css")));
    let served = backend.clone();
    app.connect_startup(move |_| {
        // The service lives as long as the process; a name already taken (a second copy) isn't an error.
        let service = service::Service { backend: served.clone() };
        match async_io::block_on(zbus::connection::Builder::session().name("org.autark.Contacts1").serve_at(service::PATH, service).build()) {
            Ok(connection) => std::mem::forget(connection),
            Err(err) => eprintln!("contacts: serving org.autark.Contacts1: {err}"),
        }
    });
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        window::Contacts::new(app, backend.clone()).window.present();
    });
    app.run()
}
