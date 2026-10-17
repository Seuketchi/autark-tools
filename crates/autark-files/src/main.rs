//! Files: browse, open, copy, move and trash files, with icon and list views, an info pane and
//! search. `autark-files [folder…]` opens a window on each folder, or on the home folder.

mod drives;
mod entry;
mod icons;
mod info;
mod ops;
mod search;
mod sidebar;
mod state;
mod thumbnails;
mod views;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use window::{Location, Window};

/// What every window shares.
pub struct App {
    pub state: RefCell<state::State>,
    pub clipboard: RefCell<Option<Clip>>,
    pub thumbnails: thumbnails::Thumbnails,
    /// Open windows, kept here: everything else holds them weakly.
    pub windows: RefCell<Vec<Rc<Window>>>,
}

#[derive(Clone)]
pub struct Clip {
    pub paths: Vec<PathBuf>,
    /// Paste moves rather than copies.
    pub cut: bool,
}

impl App {
    pub fn each_window(&self, run: fn(&Window)) {
        let windows: Vec<Rc<Window>> = self.windows.borrow().clone();
        for window in windows {
            run(&window);
        }
    }
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Files").flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    let shared = Rc::new(App { state: RefCell::new(state::State::load()), clipboard: RefCell::default(), thumbnails: Default::default(), windows: RefCell::default() });
    app.connect_startup(|app| {
        load_css();
        for (action, keys) in [
            ("win.new-window", &["<Control>n"][..]),
            ("win.close", &["<Control>w"]),
            ("win.find", &["<Control>f"]),
            ("win.show-hidden", &["<Control>h"]),
            ("win.view-grid", &["<Control>1"]),
            ("win.view-list", &["<Control>2"]),
            ("win.new-folder", &["<Control><Shift>n"]),
            ("win.back", &["<Alt>Left", "<Control>bracketleft"]),
            ("win.up", &["<Alt>Up"]),
        ] {
            app.set_accels_for_action(action, keys);
        }
    });
    app.connect_command_line(move |app, command_line| {
        let folders: Vec<PathBuf> = command_line.arguments().iter().skip(1).map(|argument| command_line.create_file_for_arg(argument)).filter_map(|file| file.path()).collect();
        let folders = if folders.is_empty() { vec![glib::home_dir()] } else { folders };
        for folder in folders {
            // A file opens its folder with the file selected.
            let location = if folder.is_dir() { folder } else { folder.parent().map_or_else(glib::home_dir, PathBuf::from) };
            Window::new(app, &shared, Location::Folder(location)).window.present();
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}

fn load_css() {
    autark_common::theme::install(include_str!("files.css"));
}
