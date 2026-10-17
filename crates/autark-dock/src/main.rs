//! Dock with magnification and running-app indicators.

mod config;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use autark_common::toplevels::{self, Toplevels, Window};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

/// Space between icons, as in the design.
const GAP: i32 = 8;
/// How far magnification spreads, in icons either side of the pointer.
const SPREAD: f64 = 2.2;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.autark.Dock")
        .build();
    app.connect_startup(|_| load_css());
    app.connect_activate(start);
    app.run()
}

/// One icon: a pinned app, or a running app that isn't pinned.
struct App {
    /// The desktop file name, or the window's app ID when no desktop entry matches.
    id: String,
    info: Option<gio::AppInfo>,
    pinned: bool,
    windows: Vec<Window>,
}

struct Dock {
    config: config::Dock,
    window: gtk::ApplicationWindow,
    panel: gtk::Box,
    /// One per app, in order, with the separator before the first unpinned app.
    icons: RefCell<Vec<gtk::Image>>,
    apps: RefCell<Vec<App>>,
    toplevels: RefCell<Option<Toplevels>>,
    /// The pointer's x in the window while it's over the dock.
    pointer: Cell<Option<f64>>,
    desktop_ids: RefCell<HashMap<String, Option<String>>>,
}

fn start(app: &gtk::Application) {
    if app.active_window().is_some() {
        return;
    }
    let mut config: config::Dock = autark_common::config::load("dock");
    config.size = config.size.clamp(32, 80);

    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_namespace(Some("autark-dock"));
    window.set_anchor(Edge::Bottom, true);
    window.set_margin(Edge::Bottom, 6);
    window.add_css_class("dock");

    let panel = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    panel.add_css_class("panel");
    panel.set_halign(gtk::Align::Center);
    panel.set_valign(gtk::Align::End);
    window.set_child(Some(&panel));

    let dock = Rc::new(Dock {
        config,
        window,
        panel,
        icons: RefCell::default(),
        apps: RefCell::default(),
        toplevels: RefCell::default(),
        pointer: Cell::new(None),
        desktop_ids: RefCell::default(),
    });

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let dock = Rc::downgrade(&dock);
        move |_, x, _| {
            if let Some(dock) = dock.upgrade() {
                dock.pointer.set(Some(x));
                dock.magnify();
            }
        }
    });
    motion.connect_leave({
        let dock = Rc::downgrade(&dock);
        move |_| {
            if let Some(dock) = dock.upgrade() {
                dock.pointer.set(None);
                dock.magnify();
            }
        }
    });
    dock.window.add_controller(motion);

    let watched = Rc::downgrade(&dock);
    match toplevels::watch(move |windows| {
        if let Some(dock) = watched.upgrade() {
            dock.update(windows);
        }
    }) {
        Ok(toplevels) => *dock.toplevels.borrow_mut() = Some(toplevels),
        Err(err) => {
            eprintln!("dock: no running-app indicators: {err}");
            dock.update(&[]);
        }
    }
    dock.window.present();
}

impl Dock {
    /// Rebuilds the icons from the pinned list and the open windows.
    fn update(self: &Rc<Self>, windows: &[Window]) {
        let installed = gio::AppInfo::all();
        let find = |id: &str| installed.iter().find(|info| info.id().is_some_and(|known| known == id)).cloned();

        let mut apps: Vec<App> = self
            .config
            .pinned
            .iter()
            .map(|id| if id.ends_with(".desktop") { id.clone() } else { format!("{id}.desktop") })
            .filter_map(|id| Some(App { info: Some(find(&id)?), id, pinned: true, windows: Vec::new() }))
            .collect();
        for window in windows.iter().filter(|window| !window.app_id.is_empty()) {
            let id = self
                .desktop_ids
                .borrow_mut()
                .entry(window.app_id.clone())
                .or_insert_with(|| autark_common::desktop::desktop_id(&window.app_id))
                .clone()
                .unwrap_or_else(|| window.app_id.clone());
            match apps.iter_mut().find(|app| app.id == id) {
                Some(app) => app.windows.push(window.clone()),
                None => apps.push(App { info: find(&id), id, pinned: false, windows: vec![window.clone()] }),
            }
        }

        while let Some(child) = self.panel.first_child() {
            self.panel.remove(&child);
        }
        let mut icons = Vec::new();
        for (index, app) in apps.iter().enumerate() {
            if !app.pinned && apps.get(index.wrapping_sub(1)).is_some_and(|before| before.pinned) {
                let separator = gtk::Box::new(gtk::Orientation::Vertical, 0);
                separator.add_css_class("separator");
                separator.set_size_request(1, self.config.size * 85 / 100);
                separator.set_valign(gtk::Align::End);
                self.panel.append(&separator);
            }
            let (item, icon) = self.item(app, index);
            self.panel.append(&item);
            icons.push(icon);
        }
        *self.icons.borrow_mut() = icons;
        *self.apps.borrow_mut() = apps;

        // The window is sized for the dock at its most magnified, so it never resizes under the
        // pointer; only the dock itself takes clicks.
        let count = self.icons.borrow().len() as i32;
        let width = count * (self.config.size + GAP) + self.config.size * 2 + 40;
        let height = self.config.size * 3 / 2 + 30;
        self.window.set_default_size(width, height);
        self.window.set_size_request(width, height);
        let reserved = self.config.size + 30;
        self.window.set_exclusive_zone(reserved);
        self.magnify();
    }

    fn item(self: &Rc<Self>, app: &App, index: usize) -> (gtk::Box, gtk::Image) {
        let icon = match &app.info {
            Some(info) => info.icon().map(|icon| gtk::Image::from_gicon(&icon)),
            None => None,
        }
        .unwrap_or_else(|| gtk::Image::from_icon_name("application-x-executable"));
        icon.set_pixel_size(self.config.size);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.set_halign(gtk::Align::Center);

        let item = gtk::Box::new(gtk::Orientation::Vertical, 0);
        item.set_valign(gtk::Align::End);
        item.append(&icon);
        item.append(&dot);
        if !app.windows.is_empty() {
            item.add_css_class("running");
        }
        let name = app.info.as_ref().map(|info| info.display_name().to_string()).unwrap_or_else(|| app.id.clone());
        item.set_tooltip_text(Some(&name));

        let click = gtk::GestureClick::builder().button(0).build();
        click.connect_released({
            let dock = Rc::downgrade(self);
            move |gesture, _, _, _| {
                if let Some(dock) = dock.upgrade() {
                    // Middle-click opens another window of the app.
                    dock.open(index, gesture.current_button() == gdk::BUTTON_MIDDLE);
                }
            }
        });
        item.add_controller(click);
        (item, icon)
    }

    /// Focuses the app's window, the next one if it already has focus, or launches it.
    fn open(&self, index: usize, new_window: bool) {
        let apps = self.apps.borrow();
        let Some(app) = apps.get(index) else {
            return;
        };
        if new_window || app.windows.is_empty() {
            let Some(info) = &app.info else {
                return;
            };
            let context = WidgetExt::display(&self.window).app_launch_context();
            if let Err(err) = info.launch(&[], Some(&context)) {
                eprintln!("dock: launching {}: {err}", app.id);
            }
            return;
        }
        let focused = app.windows.iter().position(|window| window.activated);
        let next = match focused {
            Some(position) => (position + 1) % app.windows.len(),
            None => 0,
        };
        if Some(next) != focused
            && let Some(toplevels) = &*self.toplevels.borrow()
        {
            toplevels.activate(app.windows[next].id);
        }
    }

    /// Sizes the icons for the pointer's position, measured against the unmagnified layout so
    /// growing icons don't move what's under the pointer.
    fn magnify(&self) {
        let icons = self.icons.borrow();
        let size = f64::from(self.config.size);
        let slot = size + f64::from(GAP);
        let centre = f64::from(self.window.width()) / 2.0;
        let start = centre - slot * icons.len() as f64 / 2.0;
        let hovered = self.pointer.get().filter(|_| self.config.magnify).map(|x| (x - start) / slot - 0.5);
        for (index, icon) in icons.iter().enumerate() {
            let closeness = hovered.map_or(0.0, |hovered| (1.0 - (index as f64 - hovered).abs() / SPREAD).max(0.0));
            icon.set_pixel_size((size * (1.0 + 0.5 * closeness)).round() as i32);
        }
        drop(icons);
        // Once GTK has laid out the new sizes, clicks pass through everywhere but the dock.
        let (window, panel) = (self.window.downgrade(), self.panel.downgrade());
        glib::idle_add_local_once(move || {
            if let (Some(window), Some(panel)) = (window.upgrade(), panel.upgrade()) {
                set_input_region(&window, &panel);
            }
        });
    }
}

fn set_input_region(window: &gtk::ApplicationWindow, panel: &gtk::Box) {
    let (Some(surface), Some(bounds)) = (window.surface(), panel.compute_bounds(window)) else {
        return;
    };
    let rectangle = gtk::cairo::RectangleInt::new(
        bounds.x() as i32,
        bounds.y() as i32,
        bounds.width().ceil() as i32,
        bounds.height().ceil() as i32,
    );
    surface.set_input_region(Some(&gtk::cairo::Region::create_rectangle(&rectangle)));
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        "{}{}",
        autark_common::theme::LINEN.gtk_css(),
        include_str!("dock.css")
    ));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
