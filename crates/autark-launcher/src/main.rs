//! Spotlight-style launcher, clipboard history (Super+V) and Kith's entry point.
//!
//! Runs once and stays resident: each later `autark-launcher` opens or closes it, so the
//! compositor binds Super+Space to that. `--daemon` starts it hidden, for login.

mod actions;
mod search;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use search::{Hit, Icon, Index, Target};

fn main() -> glib::ExitCode {
    let daemon = std::env::args().any(|arg| arg == "--daemon");
    let app = gtk::Application::builder()
        .application_id("org.autark.Launcher")
        .build();
    app.connect_startup(|_| load_css());
    // Started hidden, the first activation is the daemon's own and shows nothing.
    let skip = Cell::new(daemon);
    let launcher: RefCell<Option<Rc<Launcher>>> = RefCell::default();
    app.connect_activate(move |app| {
        let launcher = launcher.borrow_mut().get_or_insert_with(|| Launcher::new(app)).clone();
        if !skip.replace(false) {
            launcher.toggle();
        }
    });
    // GTK would read our flags as files to open.
    app.run_with_args(&[] as &[&str])
}

struct Launcher {
    window: gtk::ApplicationWindow,
    entry: gtk::Entry,
    results: gtk::Box,
    scroller: gtk::ScrolledWindow,
    index: RefCell<Index>,
    hits: RefCell<Vec<Hit>>,
    rows: RefCell<Vec<gtk::Box>>,
    selected: Cell<usize>,
}

impl Launcher {
    fn new(app: &gtk::Application) -> Rc<Self> {
        // Resident between openings.
        std::mem::forget(app.hold());

        let window = gtk::ApplicationWindow::new(app);
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("autark-launcher"));
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window.add_css_class("launcher");

        let search_icon = gtk::Image::from_icon_name("system-search-symbolic");
        search_icon.set_pixel_size(20);
        search_icon.add_css_class("search-icon");
        let entry = gtk::Entry::builder()
            .placeholder_text("Search apps, files and actions")
            .hexpand(true)
            .build();
        let esc = gtk::Label::new(Some("esc"));
        esc.add_css_class("key");
        esc.set_valign(gtk::Align::Center);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        header.add_css_class("header");
        header.append(&search_icon);
        header.append(&entry);
        header.append(&esc);

        let results = gtk::Box::new(gtk::Orientation::Vertical, 2);
        results.add_css_class("results");

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        footer.add_css_class("footer");
        for hint in ["↑↓ to move", "↵ to open", "Super + Space to toggle"] {
            footer.append(&gtk::Label::new(Some(hint)));
        }

        let dialog = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dialog.add_css_class("dialog");
        dialog.set_halign(gtk::Align::Center);
        dialog.set_valign(gtk::Align::Start);
        dialog.append(&header);
        // Long result lists scroll instead of running off small screens.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(520)
            .child(&results)
            .build();
        dialog.append(&scroller);
        dialog.append(&footer);

        let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
        backdrop.add_css_class("backdrop");
        backdrop.append(&dialog);
        window.set_child(Some(&backdrop));

        let launcher = Rc::new(Self {
            window,
            entry,
            results,
            scroller,
            index: RefCell::new(Index::load()),
            hits: RefCell::default(),
            rows: RefCell::default(),
            selected: Cell::new(0),
        });

        launcher.entry.connect_changed({
            let launcher = Rc::downgrade(&launcher);
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.search();
                }
            }
        });
        launcher.entry.connect_activate({
            let launcher = Rc::downgrade(&launcher);
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.open(launcher.selected.get());
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let launcher = Rc::downgrade(&launcher);
            move |_, key, _, _| {
                let Some(launcher) = launcher.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let count = launcher.hits.borrow().len();
                match key {
                    gdk::Key::Escape => launcher.hide(),
                    gdk::Key::Down if count > 0 => launcher.select((launcher.selected.get() + 1) % count),
                    gdk::Key::Up if count > 0 => launcher.select((launcher.selected.get() + count - 1) % count),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        launcher.window.add_controller(keys);

        // A click outside the dialog closes the launcher.
        let outside = gtk::GestureClick::new();
        outside.connect_released({
            let (launcher, dialog) = (Rc::downgrade(&launcher), dialog.downgrade());
            move |_, _, x, y| {
                let (Some(launcher), Some(dialog)) = (launcher.upgrade(), dialog.upgrade()) else {
                    return;
                };
                let inside = dialog
                    .compute_bounds(&launcher.window)
                    .is_some_and(|bounds| bounds.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)));
                if !inside {
                    launcher.hide();
                }
            }
        });
        backdrop.add_controller(outside);
        launcher
    }

    fn toggle(self: &Rc<Self>) {
        if self.window.is_visible() {
            self.hide();
            return;
        }
        self.index.borrow_mut().refresh();
        self.entry.set_text("");
        self.search();
        self.window.present();
        self.entry.grab_focus();
    }

    fn hide(&self) {
        self.window.set_visible(false);
    }

    fn search(self: &Rc<Self>) {
        let hits = self.index.borrow().search(&self.entry.text());
        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        let mut rows = Vec::new();
        for (index, hit) in hits.iter().enumerate() {
            if index == 0 || hits[index - 1].group != hit.group {
                let heading = gtk::Label::builder().label(hit.group.heading()).xalign(0.0).build();
                heading.add_css_class("group");
                self.results.append(&heading);
            }
            let row = self.row(hit, index);
            self.results.append(&row);
            rows.push(row);
        }
        let query = self.entry.text();
        if hits.is_empty() && !query.trim().is_empty() {
            let empty = gtk::Label::new(Some(&format!("No matches for “{}”", query.trim())));
            empty.add_css_class("empty");
            self.results.append(&empty);
        }
        self.results.set_visible(self.results.first_child().is_some());
        *self.rows.borrow_mut() = rows;
        *self.hits.borrow_mut() = hits;
        self.select(0);
    }

    fn row(self: &Rc<Self>, hit: &Hit, index: usize) -> gtk::Box {
        let icon: gtk::Widget = match &hit.icon {
            Icon::Gicon(icon) => {
                let image = gtk::Image::from_gicon(icon);
                image.set_pixel_size(32);
                image.upcast()
            }
            Icon::Glyph(name) => {
                let image = gtk::Image::from_icon_name(name);
                image.set_pixel_size(18);
                let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                tile.add_css_class("glyph");
                tile.set_valign(gtk::Align::Center);
                tile.set_homogeneous(true);
                tile.append(&image);
                tile.upcast()
            }
        };
        let title = gtk::Label::builder().label(&hit.title).xalign(0.0).build();
        title.add_css_class("title");
        let detail = gtk::Label::builder()
            .label(&hit.detail)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        detail.add_css_class("detail");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&title);
        text.append(&detail);
        let open = gtk::Label::new(Some("↵ open"));
        open.add_css_class("open");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("row");
        row.append(&icon);
        row.append(&text);
        row.append(&open);

        let hover = gtk::EventControllerMotion::new();
        hover.connect_enter({
            let launcher = Rc::downgrade(self);
            move |_, _, _| {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.select(index);
                }
            }
        });
        row.add_controller(hover);
        let click = gtk::GestureClick::new();
        click.connect_released({
            let launcher = Rc::downgrade(self);
            move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let Some(launcher) = launcher.upgrade() {
                    launcher.open(index);
                }
            }
        });
        row.add_controller(click);
        row
    }

    fn select(&self, index: usize) {
        self.selected.set(index);
        for (position, row) in self.rows.borrow().iter().enumerate() {
            if position == index {
                row.add_css_class("selected");
                self.scroll_to(row);
            } else {
                row.remove_css_class("selected");
            }
        }
    }

    /// Keeps the selected row in view; the entry keeps the keyboard, so GTK won't scroll for us.
    fn scroll_to(&self, row: &gtk::Box) {
        let Some(bounds) = row.compute_bounds(&self.results) else {
            return;
        };
        let adjustment = self.scroller.vadjustment();
        let (top, bottom) = (f64::from(bounds.y()), f64::from(bounds.y() + bounds.height()));
        if top < adjustment.value() {
            adjustment.set_value(top);
        } else if bottom > adjustment.value() + adjustment.page_size() {
            adjustment.set_value(bottom - adjustment.page_size());
        }
    }

    fn open(&self, index: usize) {
        let hits = self.hits.borrow();
        let Some(hit) = hits.get(index) else {
            return;
        };
        let context = WidgetExt::display(&self.window).app_launch_context();
        match &hit.target {
            Target::App(app) => {
                if let Err(err) = app.launch(&[], Some(&context)) {
                    eprintln!("launcher: opening {}: {err}", hit.title);
                }
                self.index.borrow_mut().opened(app);
            }
            Target::File(uri) => {
                if let Err(err) = gio::AppInfo::launch_default_for_uri(uri, Some(&context)) {
                    eprintln!("launcher: opening {uri}: {err}");
                }
            }
            Target::Action(action) => {
                let action = *action;
                glib::spawn_future_local(async move {
                    if let Err(err) = action.run().await {
                        eprintln!("launcher: {}: {err}", action.title());
                    }
                });
            }
        }
        drop(hits);
        self.hide();
    }
}

fn load_css() {
    autark_common::theme::install(include_str!("launcher.css"));
}
