//! Alt+Tab as a still row of app icons, one per app, most recently used first, with the selected app's name and window
//! title underneath. Nothing slides or fades: it's drawn where it ends up. Wayfire runs `autark-switcher next` or
//! `prev` on Alt+Tab and Alt+Shift+Tab; the first press selects the previous app, later ones move along the row, and
//! letting go of Alt switches. A tap that's over before the row would be noticed switches without drawing it.
//! Wayfire has no history to ask, so the first run stays resident, and later runs hand it their command.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use autark_common::toplevels::{self, Toplevels, Window};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

/// How long a press has to last before the row is drawn.
const DRAW_AFTER: Duration = Duration::from_millis(120);
/// If the row never gets the keyboard, Alt's release can't be seen: switch rather than leave it up.
const FOCUS_WITHIN: Duration = Duration::from_millis(600);

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Switcher").flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("switcher.css")));
    app.connect_command_line(|app, line| {
        let switcher = SWITCHER.with(|slot| slot.borrow().clone()).unwrap_or_else(|| {
            let switcher = Switcher::new(app);
            SWITCHER.set(Some(switcher.clone()));
            switcher
        });
        match line.arguments().get(1).and_then(|argument| argument.to_str()) {
            Some("next") => switcher.step(1),
            Some("prev") => switcher.step(-1),
            Some(other) => {
                eprintln!("autark-switcher: unknown command {other:?} (use next or prev)");
                return glib::ExitCode::FAILURE;
            }
            None => {}
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}

thread_local!(static SWITCHER: RefCell<Option<Rc<Switcher>>> = const { RefCell::new(None) });

/// One tile: an app, with its open windows, the one used last first.
struct Entry {
    name: String,
    icon: Option<gio::Icon>,
    windows: Vec<Window>,
}

struct Switcher {
    window: gtk::ApplicationWindow,
    panel: gtk::Box,
    row: gtk::Box,
    name: gtk::Label,
    title: gtk::Label,
    /// Open window IDs, the one used last first.
    recent: RefCell<Vec<u32>>,
    windows: RefCell<Vec<Window>>,
    toplevels: RefCell<Option<Toplevels>>,
    desktop_ids: RefCell<HashMap<String, Option<String>>>,
    entries: RefCell<Vec<Entry>>,
    tiles: RefCell<Vec<gtk::Box>>,
    selected: Cell<usize>,
    open: Cell<bool>,
    /// Counts each time the row opens or closes, so a timer from an earlier one can tell it's stale.
    epoch: Cell<u32>,
    _hold: gio::ApplicationHoldGuard,
}

impl Switcher {
    fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::new(app);
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("autark-switcher"));
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window.add_css_class("switcher");

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.add_css_class("panel");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::Center);
        let name = gtk::Label::new(None);
        name.add_css_class("name");
        let title = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(48).build();
        title.add_css_class("title");
        panel.append(&row);
        panel.append(&name);
        panel.append(&title);
        window.set_child(Some(&panel));

        let switcher = Rc::new(Self {
            window,
            panel,
            row,
            name,
            title,
            recent: RefCell::default(),
            windows: RefCell::default(),
            toplevels: RefCell::default(),
            desktop_ids: RefCell::default(),
            entries: RefCell::default(),
            tiles: RefCell::default(),
            selected: Cell::new(0),
            open: Cell::new(false),
            epoch: Cell::new(0),
            _hold: app.hold(),
        });

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_released({
            let switcher = Rc::downgrade(&switcher);
            move |_, key, _, _| {
                if matches!(key, gdk::Key::Alt_L | gdk::Key::Alt_R | gdk::Key::Meta_L | gdk::Key::Meta_R)
                    && let Some(switcher) = switcher.upgrade()
                {
                    switcher.commit();
                }
            }
        });
        keys.connect_key_pressed({
            let switcher = Rc::downgrade(&switcher);
            move |_, key, _, _| {
                let Some(switcher) = switcher.upgrade() else { return glib::Propagation::Proceed };
                match key {
                    gdk::Key::Escape => switcher.close(),
                    gdk::Key::Return | gdk::Key::KP_Enter => switcher.commit(),
                    gdk::Key::Right => switcher.step(1),
                    gdk::Key::Left => switcher.step(-1),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        switcher.window.add_controller(keys);
        // Alt may already be up by the time the row has the keyboard: a tap, then nothing left to wait for.
        switcher.window.connect_is_active_notify({
            let switcher = Rc::downgrade(&switcher);
            move |window| {
                if window.is_active()
                    && let Some(switcher) = switcher.upgrade()
                    && switcher.open.get()
                    && !alt_held()
                {
                    switcher.commit();
                }
            }
        });

        let watched = Rc::downgrade(&switcher);
        match toplevels::watch(move |windows| {
            if let Some(switcher) = watched.upgrade() {
                switcher.seen(windows);
            }
        }) {
            Ok(toplevels) => *switcher.toplevels.borrow_mut() = Some(toplevels),
            Err(err) => eprintln!("autark-switcher: can't reach the compositor (is foreign-toplevel on in Wayfire?): {err}"),
        }
        switcher
    }

    /// Keeps the use order: windows that closed go, new ones join at the back, and the focused one moves to the front.
    fn seen(&self, windows: &[Window]) {
        let mut recent = self.recent.borrow_mut();
        recent.retain(|id| windows.iter().any(|window| window.id == *id));
        for window in windows {
            if !recent.contains(&window.id) {
                recent.push(window.id);
            }
        }
        if let Some(focused) = windows.iter().find(|window| window.activated)
            && let Some(position) = recent.iter().position(|id| *id == focused.id)
        {
            let id = recent.remove(position);
            recent.insert(0, id);
        }
        *self.windows.borrow_mut() = windows.to_vec();
    }

    /// One tile per app, ordered by the app's most recently used window.
    fn entries(&self) -> Vec<Entry> {
        let windows = self.windows.borrow();
        let installed = gio::AppInfo::all();
        let mut entries: Vec<(String, Entry)> = Vec::new();
        for id in self.recent.borrow().iter() {
            let Some(window) = windows.iter().find(|window| window.id == *id).filter(|window| !window.app_id.is_empty()) else { continue };
            let key = self.desktop_ids.borrow_mut().entry(window.app_id.clone()).or_insert_with(|| autark_common::desktop::desktop_id(&window.app_id)).clone().unwrap_or_else(|| window.app_id.clone());
            match entries.iter_mut().find(|(known, _)| *known == key) {
                Some((_, entry)) => entry.windows.push(window.clone()),
                None => {
                    let info = installed.iter().find(|info| info.id().is_some_and(|known| known == key)).cloned();
                    let name = info.as_ref().map(|info| info.display_name().to_string()).unwrap_or_else(|| window.app_id.clone());
                    entries.push((key, Entry { name, icon: info.and_then(|info| info.icon()), windows: vec![window.clone()] }));
                }
            }
        }
        entries.into_iter().map(|(_, entry)| entry).collect()
    }

    /// Moves the selection along the row, opening it first if this is the first press.
    fn step(self: &Rc<Self>, delta: isize) {
        if !self.open.get() {
            let entries = self.entries();
            if entries.len() < 2 {
                return;
            }
            self.build(&entries);
            *self.entries.borrow_mut() = entries;
            self.selected.set(0);
            self.open.set(true);
            self.epoch.set(self.epoch.get().wrapping_add(1));
            self.panel.add_css_class("waiting");
            self.window.present();
            self.after(DRAW_AFTER, |switcher| switcher.panel.remove_css_class("waiting"));
            self.after(FOCUS_WITHIN, |switcher| {
                if switcher.open.get() && !switcher.window.is_active() {
                    eprintln!("autark-switcher: the row never got the keyboard, so Alt's release can't be seen; switching now");
                    switcher.commit();
                }
            });
        } else {
            // Moving on shows the row at once.
            self.panel.remove_css_class("waiting");
        }
        let count = self.entries.borrow().len() as isize;
        self.selected.set((self.selected.get() as isize + delta).rem_euclid(count) as usize);
        self.select();
    }

    fn build(self: &Rc<Self>, entries: &[Entry]) {
        while let Some(child) = self.row.first_child() {
            self.row.remove(&child);
        }
        let mut tiles = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let icon = match &entry.icon {
                Some(icon) => gtk::Image::from_gicon(icon),
                None => gtk::Image::from_icon_name("application-x-executable"),
            };
            icon.set_pixel_size(64);
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
            tile.add_css_class("tile");
            tile.append(&icon);
            let click = gtk::GestureClick::new();
            click.connect_released({
                let switcher = Rc::downgrade(self);
                move |_, _, _, _| {
                    if let Some(switcher) = switcher.upgrade() {
                        switcher.selected.set(index);
                        switcher.commit();
                    }
                }
            });
            tile.add_controller(click);
            self.row.append(&tile);
            tiles.push(tile);
        }
        *self.tiles.borrow_mut() = tiles;
    }

    fn select(&self) {
        let selected = self.selected.get();
        for (index, tile) in self.tiles.borrow().iter().enumerate() {
            if index == selected {
                tile.add_css_class("selected");
            } else {
                tile.remove_css_class("selected");
            }
        }
        if let Some(entry) = self.entries.borrow().get(selected) {
            self.name.set_text(&entry.name);
            let title = entry.windows.first().map(|window| window.title.as_str()).filter(|title| *title != entry.name).unwrap_or_default();
            self.title.set_text(title);
            self.title.set_visible(!title.is_empty());
        }
    }

    /// Switches to the selected app's window used last, and closes the row.
    fn commit(&self) {
        if !self.open.get() {
            return;
        }
        let target = self.entries.borrow().get(self.selected.get()).and_then(|entry| entry.windows.first().map(|window| window.id));
        self.close();
        if let (Some(id), Some(toplevels)) = (target, &*self.toplevels.borrow()) {
            toplevels.activate(id);
        }
    }

    fn close(&self) {
        self.open.set(false);
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.window.set_visible(false);
    }

    /// Runs `then` once, unless the row has closed by then.
    fn after(self: &Rc<Self>, delay: Duration, then: impl Fn(&Switcher) + 'static) {
        let (switcher, epoch) = (Rc::downgrade(self), self.epoch.get());
        glib::timeout_add_local_once(delay, move || {
            if let Some(switcher) = switcher.upgrade()
                && switcher.epoch.get() == epoch
            {
                then(&switcher);
            }
        });
    }
}

fn alt_held() -> bool {
    gdk::Display::default().and_then(|display| display.default_seat()).and_then(|seat| seat.keyboard()).is_some_and(|keyboard| keyboard.modifier_state().contains(gdk::ModifierType::ALT_MASK))
}
