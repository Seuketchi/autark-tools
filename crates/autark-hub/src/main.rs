//! Software: find, install, update and remove packages from a window. It reads `autark-pkg catalog` and changes things
//! only through `autark-hub-helper`, which runs `autark-pkg`, so the signature checks and the rules are `autark-pkg`'s.
//! `autark-hub <package>` opens with that package chosen.

mod catalog;
mod runner;
mod window;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Hub").flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    let slot: Rc<RefCell<Option<Rc<window::Hub>>>> = Rc::default();
    app.connect_startup(|_| autark_common::theme::install(include_str!("hub.css")));
    app.connect_command_line(move |app, command_line| {
        let wanted = command_line.arguments().get(1).and_then(|argument| argument.to_str().map(str::to_owned));
        let existing = slot.borrow().clone();
        let hub = match existing {
            Some(hub) => {
                if let Some(name) = wanted {
                    hub.choose(name);
                }
                hub
            }
            None => {
                let created = window::Hub::new(app);
                if let Some(name) = wanted {
                    created.choose(name);
                }
                *slot.borrow_mut() = Some(created.clone());
                created
            }
        };
        hub.window.present();
        glib::ExitCode::SUCCESS
    });
    app.run()
}
