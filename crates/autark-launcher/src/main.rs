//! Spotlight-style launcher, clipboard history (Super+V) and Kith's entry point: with AI on, results end
//! with Ask Kith, which turns the launcher into Kith's view (see `kith`).
//!
//! Runs once and stays resident: each later `autark-launcher` opens or closes it, so the
//! compositor binds Super+Space to that. `--daemon` starts it hidden, for login. `--clipboard` opens it on the clipboard history (Super+V), `--apps` opens it on the Apps view
//! (every installed app), or closes it if that is showing.

mod actions;
mod ai;
mod apps;
mod assistant;
mod clipboard;
mod config;
mod face;
mod kith;
mod search;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use search::{Hit, Icon, Index, Target};

fn main() -> glib::ExitCode {
    let daemon = std::env::args().any(|arg| arg == "--daemon");
    let assistant = std::env::args().any(|arg| arg == "--assistant");
    let apps = std::env::args().any(|arg| arg == "--apps");
    let clipboard = std::env::args().any(|arg| arg == "--clipboard");
    let app = gtk::Application::builder()
        .application_id("org.autark.Launcher")
        .build();
    app.connect_startup(|_| load_css());
    let launcher: Rc<RefCell<Option<Rc<Launcher>>>> = Rc::default();
    let get = {
        let launcher = launcher.clone();
        move |app: &gtk::Application| launcher.borrow_mut().get_or_insert_with(|| Launcher::new(app)).clone()
    };
    // `--assistant` opens or closes the Assistant panel, in the launcher already running if there is one.
    let toggle_assistant = gio::SimpleAction::new("assistant", None);
    toggle_assistant.connect_activate({
        let (app, get) = (app.downgrade(), get.clone());
        move |_, _| {
            let Some(app) = app.upgrade() else { return };
            let launcher = get(&app);
            // From an open launcher it carries on there, as Follow up does.
            if launcher.window.is_visible() { launcher.follow_up() } else { launcher.panel.toggle() }
        }
    });
    app.add_action(&toggle_assistant);
    // `--apps` does the same for the Apps view.
    let toggle_apps = gio::SimpleAction::new("apps", None);
    toggle_apps.connect_activate({
        let (app, get) = (app.downgrade(), get.clone());
        move |_, _| {
            let Some(app) = app.upgrade() else { return };
            get(&app).toggle_apps();
        }
    });
    app.add_action(&toggle_apps);
    // `--clipboard` does the same for the clipboard history.
    let toggle_clipboard = gio::SimpleAction::new("clipboard", None);
    toggle_clipboard.connect_activate({
        let (app, get) = (app.downgrade(), get.clone());
        move |_, _| {
            let Some(app) = app.upgrade() else { return };
            get(&app).toggle_clipboard();
        }
    });
    app.add_action(&toggle_clipboard);
    // Started hidden, the first activation is the daemon's own and shows nothing.
    let skip = Cell::new(daemon || assistant || apps || clipboard);
    app.connect_activate(move |app| {
        let launcher = get(app);
        if !skip.replace(false) {
            launcher.toggle();
        }
    });
    if assistant || apps || clipboard {
        if let Err(err) = app.register(None::<&gio::Cancellable>) {
            eprintln!("launcher: {err}");
            return glib::ExitCode::FAILURE;
        }
        app.activate_action(if assistant { "assistant" } else if clipboard { "clipboard" } else { "apps" }, None);
        if app.is_remote() {
            // The call goes out before this exits; the app is left as it is rather than torn down unrun.
            if let Some(connection) = app.dbus_connection()
                && let Err(err) = connection.flush_sync(None::<&gio::Cancellable>)
            {
                eprintln!("launcher: {err}");
            }
            std::mem::forget(app);
            return glib::ExitCode::SUCCESS;
        }
    }
    // GTK would read our flags as files to open.
    app.run_with_args(&[] as &[&str])
}

struct Launcher {
    window: gtk::ApplicationWindow,
    dialog: gtk::Box,
    search_icon: gtk::Image,
    /// Kith's face, in place of the search icon while Kith answers.
    kith_face: Rc<face::Face>,
    entry: gtk::Entry,
    results: gtk::Box,
    scroller: gtk::ScrolledWindow,
    /// Every installed app, shown in place of the results while `browsing`.
    grid: Rc<apps::Grid>,
    browsing: Cell<bool>,
    footer: gtk::Box,
    kith_hint: gtk::Label,
    kith: Rc<kith::View>,
    panel: Rc<assistant::Panel>,
    /// `ai.toml` as of opening.
    ai: Cell<kith::Settings>,
    /// Showing Kith's view rather than results.
    asking: Cell<bool>,
    index: RefCell<Index>,
    /// What was copied, newest first, and when; shown in place of the results while `clipping`.
    clips: RefCell<std::collections::VecDeque<(String, std::time::SystemTime)>>,
    clipping: Cell<bool>,
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
        let kith_face = face::Face::new(30);
        kith_face.visor.widget().set_visible(false);
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
        header.append(kith_face.visor.widget());
        let browse = gtk::Button::from_icon_name("view-app-grid-symbolic");
        browse.add_css_class("browse");
        browse.set_valign(gtk::Align::Center);
        browse.set_tooltip_text(Some("All apps"));
        header.append(&entry);
        header.append(&browse);
        header.append(&esc);

        let results = gtk::Box::new(gtk::Orientation::Vertical, 2);
        results.add_css_class("results");

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        footer.add_css_class("footer");
        let kith_hint = gtk::Label::new(Some("Tab to ask Kith"));
        for hint in ["↑↓ to move", "↵ to open"] {
            footer.append(&gtk::Label::new(Some(hint)));
        }
        footer.append(&kith_hint);
        footer.append(&gtk::Label::new(Some("Super + Space to toggle")));

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
        // Built once the launcher is, as opening an app closes the launcher.
        let opener: Rc<RefCell<std::rc::Weak<Launcher>>> = Rc::default();
        let grid = apps::Grid::new({
            let opener = opener.clone();
            move |app| {
                if let Some(launcher) = opener.borrow().upgrade() {
                    launcher.launch(app);
                }
            }
        });
        dialog.append(&grid.root);
        dialog.append(&footer);
        // Built once the launcher is, as its Open button closes the launcher.
        let hide: Rc<RefCell<std::rc::Weak<Launcher>>> = Rc::default();
        let kith = kith::View::new(
            {
                let hide = hide.clone();
                move || {
                    if let Some(launcher) = hide.borrow().upgrade() {
                        launcher.hide();
                    }
                }
            },
            {
                let hide = hide.clone();
                move || {
                    if let Some(launcher) = hide.borrow().upgrade() {
                        launcher.follow_up();
                    }
                }
            },
        );
        kith.root.set_visible(false);
        kith.footer.set_visible(false);
        dialog.insert_child_after(&kith.root, Some(&header));
        dialog.append(&kith.footer);

        let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
        backdrop.add_css_class("backdrop");
        backdrop.append(&dialog);
        window.set_child(Some(&backdrop));

        let launcher = Rc::new(Self {
            window,
            dialog: dialog.clone(),
            search_icon,
            kith_face,
            entry,
            results,
            scroller,
            grid,
            browsing: Cell::new(false),
            footer,
            kith_hint,
            kith,
            panel: assistant::Panel::new(app),
            ai: Cell::new(kith::Settings::load()),
            asking: Cell::new(false),
            index: RefCell::new(Index::load()),
            clips: RefCell::default(),
            clipping: Cell::new(false),
            hits: RefCell::default(),
            rows: RefCell::default(),
            selected: Cell::new(0),
        });
        *hide.borrow_mut() = Rc::downgrade(&launcher);
        // What's copied arrives from a thread of its own; this looks for it a few times a second.
        let copies = clipboard::watch();
        glib::timeout_add_local(std::time::Duration::from_millis(250), {
            let launcher = Rc::downgrade(&launcher);
            move || {
                let Some(launcher) = launcher.upgrade() else { return glib::ControlFlow::Break };
                let mut any = false;
                while let Ok(text) = copies.try_recv() {
                    launcher.remember(text);
                    any = true;
                }
                if any && launcher.clipping.get() && launcher.window.is_visible() {
                    launcher.search();
                }
                glib::ControlFlow::Continue
            }
        });
        *opener.borrow_mut() = Rc::downgrade(&launcher);
        browse.connect_clicked({
            let launcher = Rc::downgrade(&launcher);
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    if launcher.browsing.get() { launcher.leave_apps() } else { launcher.show_apps() }
                }
            }
        });

        launcher.entry.connect_changed({
            let launcher = Rc::downgrade(&launcher);
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    // Changing what's asked goes back to results, where Ask Kith asks again.
                    launcher.leave_kith();
                    launcher.leave_apps();
                    launcher.search();
                }
            }
        });
        launcher.entry.connect_activate({
            let launcher = Rc::downgrade(&launcher);
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    if launcher.asking.get() {
                        launcher.kith.activate();
                    } else if launcher.browsing.get() {
                        launcher.grid.focus();
                    } else {
                        launcher.open(launcher.selected.get());
                    }
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let launcher = Rc::downgrade(&launcher);
            move |_, key, _, modifiers| {
                let Some(launcher) = launcher.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
                let count = launcher.hits.borrow().len();
                match key {
                    gdk::Key::Escape => launcher.hide(),
                    gdk::Key::Return | gdk::Key::KP_Enter if control && launcher.asking.get() => launcher.follow_up(),
                    gdk::Key::Tab if !launcher.asking.get() && launcher.kith_row().is_some() => launcher.select(launcher.kith_row().unwrap_or(0)),
                    _ if launcher.asking.get() => return glib::Propagation::Proceed,
                    gdk::Key::Down if launcher.browsing.get() && launcher.entry.has_focus() => launcher.grid.focus(),
                    _ if launcher.browsing.get() => return glib::Propagation::Proceed,
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
        self.ai.set(kith::Settings::load());
        self.kith_hint.set_visible(self.ai.get().offered());
        self.leave_kith();
        self.leave_clipboard();
        self.entry.set_text("");
        self.search();
        self.window.present();
        self.entry.grab_focus();
    }

    /// Opens the launcher on the clipboard history, or closes it if that is what shows.
    fn toggle_clipboard(self: &Rc<Self>) {
        if self.window.is_visible() && self.clipping.get() {
            self.hide();
            return;
        }
        if !self.window.is_visible() {
            self.toggle();
        }
        self.leave_apps();
        self.leave_kith();
        self.clipping.set(true);
        self.kith_hint.set_visible(false);
        self.entry.set_placeholder_text(Some("Search the clipboard history"));
        self.entry.set_text("");
        self.search();
        self.entry.grab_focus();
    }

    /// Back to searching apps, files and actions.
    fn leave_clipboard(&self) {
        if self.clipping.replace(false) {
            self.entry.set_placeholder_text(Some("Search apps, files and actions"));
        }
    }

    /// Keeps a copy at the front of the history, once however many times it's copied.
    fn remember(&self, text: String) {
        let mut clips = self.clips.borrow_mut();
        clips.retain(|(kept, _)| *kept != text);
        clips.push_front((text, std::time::SystemTime::now()));
        clips.truncate(clipboard::KEEP);
    }

    /// The history as results, filtered by what's typed.
    fn clip_hits(&self, query: &str) -> Vec<Hit> {
        let query = query.trim().to_lowercase();
        let now = std::time::SystemTime::now();
        self.clips
            .borrow()
            .iter()
            .filter(|(text, _)| query.is_empty() || text.to_lowercase().contains(&query))
            .map(|(text, at)| Hit {
                group: search::Group::Clipboard,
                title: clipboard::title(text, 80),
                detail: format!("{} characters · {}", text.chars().count(), clipboard::ago(now.duration_since(*at).map_or(0, |elapsed| elapsed.as_secs()))),
                icon: Icon::Glyph("edit-paste-symbolic"),
                target: Target::Clip(text.clone()),
            })
            .collect()
    }

    /// Opens the launcher on the Apps view, or closes it if that is what shows.
    fn toggle_apps(self: &Rc<Self>) {
        if self.window.is_visible() && self.browsing.get() {
            self.hide();
            return;
        }
        if !self.window.is_visible() {
            self.toggle();
        }
        self.show_apps();
    }

    fn show_apps(self: &Rc<Self>) {
        self.leave_kith();
        self.entry.set_text("");
        self.grid.fill(self.index.borrow().all_apps());
        self.browsing.set(true);
        self.dialog.add_css_class("browsing");
        self.scroller.set_visible(false);
        self.grid.root.set_visible(true);
        self.entry.grab_focus();
    }

    /// Back to results, as typing does.
    fn leave_apps(&self) {
        if !self.browsing.replace(false) {
            return;
        }
        self.dialog.remove_css_class("browsing");
        self.grid.root.set_visible(false);
        self.scroller.set_visible(true);
    }

    fn hide(&self) {
        self.leave_clipboard();
        self.leave_apps();
        self.leave_kith();
        self.window.set_visible(false);
    }

    /// Where the Ask Kith row is.
    fn kith_row(&self) -> Option<usize> {
        self.hits.borrow().iter().position(|hit| matches!(hit.target, Target::Kith(_)))
    }

    /// Swaps results for Kith's view and asks it about `text`.
    fn ask_kith(self: &Rc<Self>, text: &str) {
        self.asking.set(true);
        self.dialog.add_css_class("asking");
        self.search_icon.set_visible(false);
        self.kith_face.visor.widget().set_visible(true);
        self.scroller.set_visible(false);
        self.footer.set_visible(false);
        self.kith.root.set_visible(true);
        self.kith.footer.set_visible(true);
        self.kith.start(text, self.ai.get());
    }

    /// Carries on from Ask Kith in the Assistant panel, the answer still being written there.
    fn follow_up(&self) {
        self.kith.detach();
        self.hide();
        self.panel.open();
    }

    fn leave_kith(&self) {
        if !self.asking.replace(false) {
            return;
        }
        self.kith.cancel();
        self.dialog.remove_css_class("asking");
        self.search_icon.set_visible(true);
        self.kith_face.visor.widget().set_visible(false);
        self.scroller.set_visible(true);
        self.footer.set_visible(true);
        self.kith.root.set_visible(false);
        self.kith.footer.set_visible(false);
    }

    fn search(self: &Rc<Self>) {
        let hits = if self.clipping.get() { self.clip_hits(&self.entry.text()) } else { self.index.borrow().search(&self.entry.text(), self.ai.get().offered()) };
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
            Icon::Kith => {
                let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                tile.add_css_class("glyph");
                tile.set_valign(gtk::Align::Center);
                tile.set_homogeneous(true);
                tile.append(&kith_mark());
                tile.upcast()
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
        let open = gtk::Label::new(Some(match hit.target {
            Target::Kith(_) => "↵ ask",
            Target::Clip(_) => "↵ copy",
            _ => "↵ open",
        }));
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

    /// Starts an app, counts the opening for ranking, and closes the launcher.
    fn launch(&self, app: &gio::AppInfo) {
        let context = WidgetExt::display(&self.window).app_launch_context();
        if let Err(err) = app.launch(&[], Some(&context)) {
            eprintln!("launcher: opening {}: {err}", app.display_name());
        }
        self.index.borrow_mut().opened(app);
        self.hide();
    }

    fn open(self: &Rc<Self>, index: usize) {
        let hits = self.hits.borrow();
        let Some(hit) = hits.get(index) else {
            return;
        };
        if let Target::Kith(text) = &hit.target {
            let text = text.clone();
            drop(hits);
            self.ask_kith(&text);
            return;
        }
        let context = WidgetExt::display(&self.window).app_launch_context();
        match &hit.target {
            Target::App(app) => {
                let app = app.clone();
                drop(hits);
                self.launch(&app);
                return;
            }
            Target::File(uri) => {
                if let Err(err) = gio::AppInfo::launch_default_for_uri(uri, Some(&context)) {
                    eprintln!("launcher: opening {uri}: {err}");
                }
            }
            Target::Kith(_) => {}
            Target::Clip(text) => {
                if let Err(err) = clipboard::copy(text.clone()) {
                    eprintln!("launcher: copying: {err}");
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

/// Kith's sparkle, as the design draws it beside "Ask Kith".
fn kith_mark() -> gtk::Label {
    let mark = gtk::Label::new(Some("✦"));
    mark.add_css_class("kith-mark");
    mark
}

fn load_css() {
    autark_common::theme::install(include_str!("launcher.css"));
}
