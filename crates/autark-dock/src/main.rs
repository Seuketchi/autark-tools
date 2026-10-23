//! Dock with magnification and running-app indicators, at the bottom or either side of the screen, shown
//! always or only when the pointer reaches its edge. `dock.toml` is watched, so Settings' changes apply at once.

mod config;
mod snapshot;

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
/// How long the pointer rests on an app with several windows before its list of them shows, so passing over the dock doesn't.
const LIST_AFTER: Duration = Duration::from_millis(250);
/// How long the list stays once the pointer has left, enough to cross the gap onto it.
const LIST_LINGER: Duration = Duration::from_millis(300);
/// How long a window has to keep focus before its picture is taken, so it has drawn what it opens with.
const SETTLE: Duration = Duration::from_millis(600);
/// A window's picture in the list, in pixels.
const THUMB: (i32, i32) = (160, 100);
/// The windows in the list sit side by side, this many to a line at most.
const PER_LINE: u32 = 4;

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
    /// The box each icon sits in, in the same order, which an app's window list hangs off.
    items: RefCell<Vec<gtk::Box>>,
    /// The list of an app's windows, shown while the pointer is on its icon, and the app it's for.
    list: RefCell<Option<(String, gtk::Popover)>>,
    /// Whether the pointer is on that list.
    on_list: Cell<bool>,
    /// Counts hovers, so a wait that began for an earlier one knows it's stale.
    hover_epoch: Cell<u32>,
    /// Each window's picture as last taken, by window ID, for the window list.
    pictures: RefCell<HashMap<u32, gdk::Texture>>,
    /// The window that had focus at the last update.
    focused: Cell<u32>,
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
        items: RefCell::default(),
        list: RefCell::default(),
        on_list: Cell::new(false),
        hover_epoch: Cell::new(0),
        pictures: RefCell::default(),
        focused: Cell::new(0),
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
    autark_common::theme::on_change(move |_| {
        if let Some(dock) = watched.upgrade() {
            dock.arrange();
            dock.magnify();
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
            dock.pictures.borrow_mut().retain(|id, _| windows.iter().any(|window| window.id == *id));
            dock.focus_moved(windows);
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
        self.revealer.set_transition_duration(if autark_common::theme::look().theme.reduce_motion { 0 } else { 180 });
        self.revealer.set_reveal_child(!config.autohide);
    }

    /// Rebuilds the icons from the pinned list and the open windows.
    fn update(self: &Rc<Self>) {
        // The list hangs off an icon that's about to be replaced; it comes back on the new one below.
        let reopen = self.close_list();
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
        let mut items = Vec::new();
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
            items.push(item);
        }
        *self.icons.borrow_mut() = icons;
        *self.items.borrow_mut() = items;
        *self.apps.borrow_mut() = apps;
        let reopened = reopen.and_then(|id| self.apps.borrow().iter().position(|app| app.id == id));
        if let Some(index) = reopened {
            self.show_list(index);
        }

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
        if app.windows.len() < 2 {
            item.set_tooltip_text(Some(&name));
        } else {
            // Its windows are listed instead, which says the name too.
            let motion = gtk::EventControllerMotion::new();
            motion.connect_enter({
                let dock = Rc::downgrade(self);
                move |_, _, _| {
                    if let Some(dock) = dock.upgrade() {
                        dock.hover(index, true);
                    }
                }
            });
            motion.connect_leave({
                let dock = Rc::downgrade(self);
                move |_| {
                    if let Some(dock) = dock.upgrade() {
                        dock.hover(index, false);
                    }
                }
            });
            item.add_controller(motion);
        }

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

    /// A window that has just taken focus gets its picture once it has settled.
    fn focus_moved(self: &Rc<Self>, windows: &[Window]) {
        let focused = windows.iter().find(|window| window.activated).map_or(0, |window| window.id);
        if self.focused.replace(focused) == focused || focused == 0 {
            return;
        }
        let dock = Rc::downgrade(self);
        glib::timeout_add_local_once(SETTLE, move || {
            if let Some(dock) = dock.upgrade()
                && dock.focused.get() == focused
            {
                dock.take_picture(focused);
            }
        });
    }

    /// Takes the focused window's picture, unless the window list is up: it would be in the picture.
    fn take_picture(&self, id: u32) {
        if self.list.borrow().is_some() {
            return;
        }
        if let Some(texture) = snapshot::focused() {
            self.pictures.borrow_mut().insert(id, texture);
        }
        // A capture is a few screens' worth of pixels, copied a few times. Freed, they would stay in the heap, and the
        // dock would keep the biggest capture it ever took (tens of megabytes) for as long as it runs.
        // SAFETY: malloc_trim only hands back free memory.
        unsafe { libc::malloc_trim(0) };
    }

    /// The pointer reached or left an icon: after a short wait, show that app's list of windows or take it away.
    fn hover(self: &Rc<Self>, index: usize, entered: bool) {
        let epoch = self.hover_epoch.get().wrapping_add(1);
        self.hover_epoch.set(epoch);
        let dock = Rc::downgrade(self);
        glib::timeout_add_local_once(if entered { LIST_AFTER } else { LIST_LINGER }, move || {
            let Some(dock) = dock.upgrade() else { return };
            if dock.hover_epoch.get() != epoch {
                return;
            }
            if entered {
                dock.show_list(index);
            } else if !dock.on_list.get() {
                dock.close_list();
            }
        });
    }

    /// Lists the app's windows by title above its icon (beside it, for a dock on a side): click one to focus it.
    fn show_list(self: &Rc<Self>, index: usize) {
        let apps = self.apps.borrow();
        let Some(app) = apps.get(index).filter(|app| app.windows.len() >= 2) else { return };
        if self.list.borrow().as_ref().is_some_and(|(id, _)| *id == app.id) {
            return;
        }
        let Some(item) = self.items.borrow().get(index).cloned() else { return };
        self.close_list();
        let name = app.info.as_ref().map(|info| info.display_name().to_string()).unwrap_or_else(|| app.id.clone());
        let (side, offset) = match self.config.borrow().position {
            Position::Bottom => (gtk::PositionType::Top, (0, -8)),
            Position::Left => (gtk::PositionType::Right, (8, 0)),
            Position::Right => (gtk::PositionType::Left, (-8, 0)),
        };
        let popover = gtk::Popover::builder().autohide(false).has_arrow(false).position(side).build();
        popover.set_offset(offset.0, offset.1);
        popover.add_css_class("windows");
        let rows = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).min_children_per_line(1).max_children_per_line(PER_LINE.min(app.windows.len() as u32)).row_spacing(4).column_spacing(4).homogeneous(true).build();
        for window in &app.windows {
            let title = if window.title.is_empty() { name.as_str() } else { window.title.as_str() };
            let label = gtk::Label::builder().label(title).xalign(0.5).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(20).build();
            // The picture, or the app's icon where the window hasn't had one taken yet.
            let thumb = gtk::Box::new(gtk::Orientation::Vertical, 0);
            thumb.add_css_class("thumb");
            thumb.set_size_request(THUMB.0, THUMB.1);
            thumb.set_overflow(gtk::Overflow::Hidden);
            match self.pictures.borrow().get(&window.id) {
                Some(texture) => {
                    let picture = gtk::Picture::builder().paintable(texture).content_fit(gtk::ContentFit::Contain).can_shrink(true).vexpand(true).build();
                    thumb.append(&picture);
                }
                None => {
                    let icon = match app.info.as_ref().and_then(|info| info.icon()) {
                        Some(icon) => gtk::Image::from_gicon(&icon),
                        None => gtk::Image::from_icon_name("application-x-executable"),
                    };
                    icon.set_pixel_size(32);
                    icon.set_vexpand(true);
                    thumb.append(&icon);
                }
            }
            let contents = gtk::Box::new(gtk::Orientation::Vertical, 6);
            contents.append(&thumb);
            contents.append(&label);
            let row = gtk::Button::builder().child(&contents).build();
            row.add_css_class("window-row");
            if window.activated {
                row.add_css_class("focused");
            }
            if window.minimized {
                row.add_css_class("minimized");
            }
            row.connect_clicked({
                let (dock, id) = (Rc::downgrade(self), window.id);
                move |_| {
                    let Some(dock) = dock.upgrade() else { return };
                    if let Some(toplevels) = &*dock.toplevels.borrow() {
                        toplevels.activate(id);
                    }
                    // Not closed here: this handler belongs to a widget the list owns.
                    let dock = Rc::downgrade(&dock);
                    glib::idle_add_local_once(move || {
                        if let Some(dock) = dock.upgrade() {
                            dock.close_list();
                        }
                    });
                }
            });
            rows.insert(&row, -1);
        }
        popover.set_child(Some(&rows));
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let dock = Rc::downgrade(self);
            move |_, _, _| {
                if let Some(dock) = dock.upgrade() {
                    dock.on_list.set(true);
                    dock.hover_epoch.set(dock.hover_epoch.get().wrapping_add(1));
                }
            }
        });
        motion.connect_leave({
            let dock = Rc::downgrade(self);
            move |_| {
                let Some(dock) = dock.upgrade() else { return };
                dock.on_list.set(false);
                let epoch = dock.hover_epoch.get().wrapping_add(1);
                dock.hover_epoch.set(epoch);
                let dock = Rc::downgrade(&dock);
                glib::timeout_add_local_once(LIST_LINGER, move || {
                    if let Some(dock) = dock.upgrade()
                        && dock.hover_epoch.get() == epoch
                        && !dock.on_list.get()
                    {
                        dock.close_list();
                        dock.hide_soon();
                    }
                });
            }
        });
        popover.add_controller(motion);
        popover.set_parent(&item);
        popover.popup();
        *self.list.borrow_mut() = Some((app.id.clone(), popover));
    }

    /// Takes the window list away, saying which app's it was.
    fn close_list(&self) -> Option<String> {
        let (id, popover) = self.list.borrow_mut().take()?;
        self.on_list.set(false);
        popover.popdown();
        popover.unparent();
        Some(id)
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
                if dock.pointer.get().is_none() && !dock.on_list.get() {
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
        // Reduced motion leaves the icons still.
        let magnify = config.magnify && !autark_common::theme::look().theme.reduce_motion;
        let hovered = along.filter(|_| magnify).map(|along| (along - start) / slot - 0.5);
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
    autark_common::theme::install(include_str!("dock.css"));
}
