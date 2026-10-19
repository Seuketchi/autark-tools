//! Mail: every account's mail in one place, over IMAP and SMTP. Accounts are signed in once, by
//! `autark-accounts`. While it runs it also serves `org.autark.Mail1` for Kith. `autark-mail daemon` serves it
//! without a window, which is how the session bus starts it when Kith asks and the app isn't open.

mod backend;
mod compose;
mod logic;
mod service;
mod window;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("daemon") {
        return daemon();
    }
    let app = gtk::Application::builder().application_id("org.autark.Mail").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("mail.css")));
    app.connect_startup(|_| {
        // The service lives as long as the process; a name already taken (a second copy) isn't an error.
        match async_io::block_on(zbus::connection::Builder::session().name("org.autark.Mail1").serve_at(service::PATH, service::Service).build()) {
            Ok(connection) => std::mem::forget(connection),
            Err(err) => eprintln!("mail: serving org.autark.Mail1: {err}"),
        }
    });
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        window::Mail::new(app).window.present();
    });
    app.run()
}

/// Serves `org.autark.Mail1` and nothing else, for as long as the session lasts.
fn daemon() -> glib::ExitCode {
    match async_io::block_on(zbus::connection::Builder::session().name("org.autark.Mail1").serve_at(service::PATH, service::Service).build()) {
        Ok(connection) => {
            std::mem::forget(connection);
            loop {
                std::thread::park();
            }
        }
        Err(err) => {
            eprintln!("mail: serving org.autark.Mail1: {err}");
            glib::ExitCode::FAILURE
        }
    }
}
