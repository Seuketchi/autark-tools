//! Activity, the task manager: apps and processes with CPU, memory, disk, network and energy
//! impact, read from `autark-stats`, and force quit. `autark-monitor --force-quit` opens just
//! the force-quit dialog; the compositor binds Ctrl+Alt+Esc to it.

mod force_quit;
mod stats;
mod window;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Activity").flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    app.connect_startup(|_| load_css());
    let monitor: RefCell<Option<Rc<window::Monitor>>> = RefCell::default();
    app.connect_command_line(move |app, command_line| {
        let monitor = monitor.borrow_mut().get_or_insert_with(|| window::Monitor::new(app)).clone();
        if command_line.arguments().iter().any(|arg| arg.to_str() == Some("--force-quit")) {
            // The list fills from the first reading, a moment after start.
            let (monitor, hold) = (monitor.clone(), app.hold());
            glib::timeout_add_local_once(std::time::Duration::from_millis(if monitor.has_snapshot() { 0 } else { 1200 }), move || {
                monitor.show_force_quit();
                drop(hold);
            });
        } else {
            monitor.show_window();
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!("{}{}", autark_common::theme::LINEN.gtk_css(), include_str!("monitor.css")));
    gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("no display"), &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}
