//! Settings: every pane edits the same config files you can (`~/.config/autark/*.toml`, `wayfire.ini`), and
//! the tools pick the change up by watching their file; the daemons are also told over D-Bus where they offer
//! it. `autark-settings [pane]` opens the window, on a pane by id (`display`, `sound`…) if given.

mod bluez;
mod bus;
mod config;
mod icon;
mod iwd;
mod panes;
mod ui;
mod wayfire;
mod window;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Settings").flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    let shell: Rc<RefCell<Option<Rc<window::Shell>>>> = Rc::default();
    app.connect_startup(|_| load_css());
    app.connect_command_line(move |app, command_line| {
        let pane = command_line.arguments().get(1).and_then(|argument| argument.to_str().map(str::to_owned));
        let shell = shell.borrow_mut().get_or_insert_with(|| window::Shell::new(app)).clone();
        match pane.as_deref() {
            Some(id) if panes::find(id).is_some() => shell.show(id),
            Some(id) => {
                let ids: Vec<&str> = panes::PANES.iter().map(|pane| pane.id).collect();
                eprintln!("settings: no pane {id:?}; the panes are {}", ids.join(", "));
                return glib::ExitCode::FAILURE;
            }
            None if shell.window.is_visible() => {}
            None => shell.show(panes::PANES[0].id),
        }
        shell.window.present();
        glib::ExitCode::SUCCESS
    });
    app.run()
}

fn load_css() {
    autark_common::theme::install(include_str!("settings.css"));
}
