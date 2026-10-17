//! Dock with magnification and running-app indicators, at the bottom or either side of the screen, shown
//! always or only when the pointer reaches its edge. `dock.toml` is watched, so Settings' changes apply at once.

mod config;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use autark_common::toplevels::{self, Toplevels, Window};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use config::Position;

/// Space between icons, as in the design.
const GAP: i32 = 8;
/// How far magnification spreads, in icons either side of the pointer.
const SPREAD: f64 = 2.2;
/// The gap between the dock and its edge of the screen.
const MARGIN: i32 = 6;
/// How long the pointer must be gone before a hiding dock goes.
const HIDE_AFTER: Duration = Duration::from_millis(400);

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Dock").build();
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
    config: RefCell<config::Dock>,
    window: gtk::ApplicationWindow,
    /// Slides the panel out of sight when it hides.
    revealer: gtk::Revealer,
    panel: gtk::Box,
    /// One per app, in order, with the separator before the first unpinned app.
    icons: RefCell<Vec<gtk::Image>>,
    apps: RefCell<Vec<App>>,
    /// The open windows as last reported, to rebuild from when the config changes.
    windows: RefCell<Vec<Window>>,
    toplevels: RefCell<Option<Toplevels>>,
    /// The pointer in the window while it's over the dock.
    pointer: Cell<Option<(f64, f64)>>,
    desktop_ids: RefCell<HashMap<String, Option<String>>>,
    hide_timer: RefCell<Option<glib::SourceId>>,
    /// The input region last set, as setting it again lays the surface out again.
    region: Rc<Cell<(i32, i32, i32, i32)>>,
    _watch: RefCell<Option<autark_common::watch::Watch>>,
}

fn start(app: &gtk::Application) {
    if app.active_window().is_some() {
        return;
    }
    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_namespace(Some("autark-dock"));
    window.add_css_class("dock");

    let panel = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    panel.add_css_class("panel");
    let revealer = gtk::Revealer::builder().child(&panel).reveal_child(true).transition_duration(180).build();
    window.set_child(Some(&revealer));

    let dock = Rc::new(Dock {
        config: RefCell::new(config::Dock::load()),
        window,
        revealer,
        panel,
        icons: RefCell::default(),
        apps: RefCell::default(),
        windows: RefCell::default(),
        toplevels: RefCell::default(),
        pointer: Cell::new(None),
        desktop_ids: RefCell::default(),
        hide_timer: RefCell::default(),
        region: Rc::default(),
        _watch: RefCell::default(),
    });

    let motion = gtk::EventControllerMotion::new();
    let moved = {
        let dock = Rc::downgrade(&dock);
        move |_: &gtk::EventControllerMotion, x: f64, y: f64| {
            if let Some(dock) = dock.upgrade() {
                dock.pointer.set(Some((x, y)));
                dock.show();
                dock.magnify();
            }
        }
    };
    motion.connect_enter(moved.clone());
    motion.connect_motion(moved);
    motion.connect_leave({
        let dock = Rc::downgrade(&dock);
        move |_| {
            if let Some(dock) = dock.upgrade() {
                dock.pointer.set(None);
                dock.magnify();
                dock.hide_soon();
            }
        }
    });
    dock.window.add_controller(motion);
    dock.revealer.connect_child_revealed_notify({
        let dock = Rc::downgrade(&dock);
        move |_| {
            if let Some(dock) = dock.upgrade() {
                dock.update_input_region();
            }
        }
    });

    // The input region depends on the window's size, which the compositor gives after it's shown.
    dock.window.connect_realize({
        let dock = Rc::downgrade(&dock);
        move |window| {
            let Some(surface) = window.surface() else { return };
            let (dock, size) = (dock.clone(), Cell::new((0, 0)));
            surface.connect_layout(move |_, width, height| {
                if size.replace((width, height)) != (width, height)
                    && let Some(dock) = dock.upgrade()
                {
                    dock.update_input_region();
                }
            });
        }
    });

    let watched = Rc::downgrade(&dock);
    *dock._watch.borrow_mut() = Some(autark_common::watch::config("dock", move || {
        if let Some(dock) = watched.upgrade() {
            dock.reload();
        }
    }));

    dock.arrange();
    let watched = Rc::downgrade(&dock);
    match toplevels::watch(move |windows| {
        if let Some(dock) = watched.upgrade() {
            *dock.windows.borrow_mut() = windows.to_vec();
            dock.update();
        }
    }) {
        Ok(toplevels) => *dock.toplevels.borrow_mut() = Some(toplevels),
        Err(err) => {
            eprintln!("dock: no running-app indicators: {err}");
            dock.update();
        }
    }
    dock.window.present();
    // Everything else holds the dock weakly; it lives as long as the program.
    DOCK.set(Some(dock));
}

thread_local!(static DOCK: RefCell<Option<Rc<Dock>>> = const { RefCell::new(None) });

impl Dock {
    /// Reads `dock.toml` again after it changed, and applies what's different.
    fn reload(self: &Rc<Self>) {
        let config = config::Dock::load();
        if *self.config.borrow() == config {
            return;
        }
        *self.config.borrow_mut() = config;
        self.arrange();
        self.update();
    }

    /// Puts the window on its edge, for the position and hiding.
    fn arrange(&self) {
        let config = self.config.borrow();
        let edge = match config.position {
            Position::Bottom => Edge::Bottom,
            Position::Left => Edge::Left,
            Position::Right => Edge::Right,
        };
        // The window spans its whole edge, so a hidden dock comes back wherever the pointer meets the edge;
        // clicks pass through everywhere but the dock (see `update_input_region`).
        let vertical = config.position != Position::Bottom;
        for side in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            let along = if vertical { matches!(side, Edge::Top | Edge::Bottom) } else { matches!(side, Edge::Left | Edge::Right) };
            self.window.set_anchor(side, side == edge || along);
            self.window.set_margin(side, 0);
        }
        self.panel.set_orientation(if vertical { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal });
        let (halign, valign, transition) = match config.position {
            Position::Bottom => (gtk::Align::Center, gtk::Align::End, gtk::RevealerTransitionType::SlideUp),
            Position::Left => (gtk::Align::Start, gtk::Align::Center, gtk::RevealerTransitionType::SlideRight),
            Position::Right => (gtk::Align::End, gtk::Align::Center, gtk::RevealerTransitionType::SlideLeft),
        };
        self.revealer.set_halign(halign);
        self.revealer.set_valign(valign);
        self.revealer.set_transition_type(transition);
        self.revealer.set_margin_bottom(if config.position == Position::Bottom { MARGIN } else { 0 });
        self.revealer.set_margin_start(if config.position == Position::Left { MARGIN } else { 0 });
        self.revealer.set_margin_end(if config.position == Position::Right { MARGIN } else { 0 });
        for (class, position) in [("bottom", Position::Bottom), ("left", Position::Left), ("right", Position::Right)] {
            if config.position == position {
                self.window.add_css_class(class);
            } else {
                self.window.remove_css_class(class);
            }
        }
        if config.dots {
            self.window.add_css_class("dots");
        } else {
            self.window.remove_css_class("dots");
        }
        self.revealer.set_reveal_child(!config.autohide);
    }

    /// Rebuilds the icons from the pinned list and the open windows.
    fn update(self: &Rc<Self>) {
        let installed = gio::AppInfo::all();
        let find = |id: &str| installed.iter().find(|info| info.id().is_some_and(|known| known == id)).cloned();
        let config = self.config.borrow().clone();

        let mut apps: Vec<App> = config
            .pinned
            .iter()
            .map(|id| if id.ends_with(".desktop") { id.clone() } else { format!("{id}.desktop") })
            .filter_map(|id| Some(App { info: Some(find(&id)?), id, pinned: true, windows: Vec::new() }))
            .collect();
        for window in self.windows.borrow().iter().filter(|window| !window.app_id.is_empty()) {
            let id = self.desktop_ids.borrow_mut().entry(window.app_id.clone()).or_insert_with(|| autark_common::desktop::desktop_id(&window.app_id)).clone().unwrap_or_else(|| window.app_id.clone());
            match apps.iter_mut().find(|app| app.id == id) {
                Some(app) => app.windows.push(window.clone()),
                None => apps.push(App { info: find(&id), id, pinned: false, windows: vec![window.clone()] }),
            }
        }

        while let Some(child) = self.panel.first_child() {
            self.panel.remove(&child);
        }
        let vertical = config.position != Position::Bottom;
        let mut icons = Vec::new();
        for (index, app) in apps.iter().enumerate() {
            if !app.pinned && apps.get(index.wrapping_sub(1)).is_some_and(|before| before.pinned) {
                let separator = gtk::Box::new(gtk::Orientation::Vertical, 0);
                separator.add_css_class("separator");
                let length = config.size * 85 / 100;
                if vertical {
                    separator.set_size_request(length, 1);
                } else {
                    separator.set_size_request(1, length);
                }
                self.panel.append(&separator);
            }
            let (item, icon) = self.item(app, index, &config);
            self.panel.append(&item);
            icons.push(icon);
        }
        *self.icons.borrow_mut() = icons;
        *self.apps.borrow_mut() = apps;

        // The window is as deep as the dock at its most magnified, so it never resizes under the pointer.
        let depth = config.size * 3 / 2 + 30;
        if vertical {
            self.window.set_default_size(depth, -1);
            self.window.set_size_request(depth, -1);
        } else {
            self.window.set_default_size(-1, depth);
            self.window.set_size_request(-1, depth);
        }
        self.window.set_exclusive_zone(if config.autohide { 0 } else { config.size + 30 });
        self.magnify();
    }

    fn item(self: &Rc<Self>, app: &App, index: usize, config: &config::Dock) -> (gtk::Box, gtk::Image) {
        let icon = match &app.info {
            Some(info) => info.icon().map(|icon| gtk::Image::from_gicon(&icon)),
            None => None,
        }
        .unwrap_or_else(|| gtk::Image::from_icon_name("application-x-executable"));
        icon.set_pixel_size(config.size);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.set_halign(gtk::Align::Center);
        dot.set_valign(gtk::Align::Center);

        // The dot sits on the screen-edge side of the icon.
        let item = match config.position {
            Position::Bottom => {
                let item = gtk::Box::new(gtk::Orientation::Vertical, 0);
                item.set_valign(gtk::Align::End);
                item.append(&icon);
                item.append(&dot);
                item
            }
            Position::Left => {
                let item = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                item.set_halign(gtk::Align::Start);
                item.append(&dot);
                item.append(&icon);
                item
            }
            Position::Right => {
                let item = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                item.set_halign(gtk::Align::End);
                item.append(&icon);
                item.append(&dot);
                item
            }
        };
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

    /// Brings a hiding dock back while the pointer is on it.
    fn show(&self) {
        if let Some(timer) = self.hide_timer.borrow_mut().take() {
            timer.remove();
        }
        self.revealer.set_reveal_child(true);
    }

    fn hide_soon(self: &Rc<Self>) {
        if !self.config.borrow().autohide {
            return;
        }
        if let Some(timer) = self.hide_timer.borrow_mut().take() {
            timer.remove();
        }
        let dock = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(HIDE_AFTER, move || {
            if let Some(dock) = dock.upgrade() {
                dock.hide_timer.borrow_mut().take();
                if dock.pointer.get().is_none() {
                    dock.revealer.set_reveal_child(false);
                    dock.update_input_region();
                }
            }
        });
        *self.hide_timer.borrow_mut() = Some(id);
    }

    /// Sizes the icons for the pointer's position, measured against the unmagnified layout so
    /// growing icons don't move what's under the pointer.
    fn magnify(&self) {
        let icons = self.icons.borrow();
        let config = self.config.borrow();
        let size = f64::from(config.size);
        let slot = size + f64::from(GAP);
        let vertical = config.position != Position::Bottom;
        let length = if vertical { self.window.height() } else { self.window.width() };
        let centre = f64::from(length) / 2.0;
        let start = centre - slot * icons.len() as f64 / 2.0;
        let along = self.pointer.get().map(|(x, y)| if vertical { y } else { x });
        let hovered = along.filter(|_| config.magnify).map(|along| (along - start) / slot - 0.5);
        for (index, icon) in icons.iter().enumerate() {
            let closeness = hovered.map_or(0.0, |hovered| (1.0 - (index as f64 - hovered).abs() / SPREAD).max(0.0));
            icon.set_pixel_size((size * (1.0 + 0.5 * closeness)).round() as i32);
        }
        drop((icons, config));
        // Once GTK has laid out the new sizes, set where clicks land.
        let window = self.window.downgrade();
        let (revealer, position, region) = (self.revealer.downgrade(), self.config.borrow().position, self.region.clone());
        glib::idle_add_local_once(move || {
            if let (Some(window), Some(revealer)) = (window.upgrade(), revealer.upgrade()) {
                set_input_region(&window, &revealer, position, &region);
            }
        });
    }

    fn update_input_region(&self) {
        set_input_region(&self.window, &self.revealer, self.config.borrow().position, &self.region);
    }
}

/// Clicks land on the dock and the gap between it and the screen edge, or, while it's hidden, on a thin strip
/// along the edge that brings it back. Everywhere else they pass through to the windows below.
fn set_input_region(window: &gtk::ApplicationWindow, revealer: &gtk::Revealer, position: Position, last: &Cell<(i32, i32, i32, i32)>) {
    let Some(surface) = window.surface() else { return };
    let (width, height) = (window.width(), window.height());
    let shown = revealer.reveals_child() || revealer.is_child_revealed();
    let rectangle = match (shown, revealer.child().and_then(|panel| panel.compute_bounds(window))) {
        (true, Some(bounds)) => {
            let (x, y, w, h) = (bounds.x() as i32, bounds.y() as i32, bounds.width().ceil() as i32, bounds.height().ceil() as i32);
            match position {
                Position::Bottom => gtk::cairo::RectangleInt::new(x, y, w, height - y),
                Position::Left => gtk::cairo::RectangleInt::new(0, y, x + w, h),
                Position::Right => gtk::cairo::RectangleInt::new(x, y, width - x, h),
            }
        }
        _ => match position {
            Position::Bottom => gtk::cairo::RectangleInt::new(0, height - 2, width, 2),
            Position::Left => gtk::cairo::RectangleInt::new(0, 0, 2, height),
            Position::Right => gtk::cairo::RectangleInt::new(width - 2, 0, 2, height),
        },
    };
    let key = (rectangle.x(), rectangle.y(), rectangle.width(), rectangle.height());
    if last.replace(key) != key {
        surface.set_input_region(Some(&gtk::cairo::Region::create_rectangle(&rectangle)));
    }
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!("{}{}", autark_common::theme::LINEN.gtk_css(), include_str!("dock.css")));
    gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("no display"), &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}
