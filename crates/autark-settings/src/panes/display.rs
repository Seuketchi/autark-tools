//! Display: the monitors drawn as arranged (drag one to move it), the chosen one's resolution, refresh rate,
//! scale and brightness or rotation, and night light. Monitors go through `org.autark.Displays1`, which
//! remembers each arrangement; brightness through logind, which may set the backlight without root.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{bus, ui};

#[zbus::proxy(interface = "org.autark.Displays1", default_service = "org.autark.Displays1", default_path = "/org/autark/Displays1")]
trait Displays {
    fn list(&self) -> zbus::Result<String>;
    fn apply(&self, outputs: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn changed(&self, heads: &str) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1", default_path = "/org/freedesktop/login1/session/auto")]
trait Session {
    fn set_brightness(&self, subsystem: &str, name: &str, brightness: u32) -> zbus::Result<()>;
}

/// `autark-displays`' monitor, as `List` gives it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Mode {
    width: i32,
    height: i32,
    /// Millihertz.
    refresh: i32,
    preferred: bool,
}

impl Mode {
    fn text(&self) -> String {
        format!("{}x{}@{:.2}", self.width, self.height, f64::from(self.refresh) / 1000.0)
    }

    fn hertz(&self) -> String {
        let hertz = f64::from(self.refresh) / 1000.0;
        if (hertz - hertz.round()).abs() < 0.05 { format!("{hertz:.0} Hz") } else { format!("{hertz:.2} Hz") }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Head {
    name: String,
    description: String,
    make: String,
    model: String,
    physical: (i32, i32),
    modes: Vec<Mode>,
    enabled: bool,
    mode: Option<Mode>,
    x: i32,
    y: i32,
    scale: f64,
    transform: String,
}

impl Head {
    fn internal(&self) -> bool {
        ["eDP", "LVDS", "DSI"].iter().any(|prefix| self.name.starts_with(prefix))
    }

    fn title(&self) -> String {
        if self.internal() {
            return "Built-in display".into();
        }
        let name = [self.make.as_str(), self.model.as_str()].iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(" ");
        if !self.model.is_empty() {
            self.model.clone()
        } else if !name.is_empty() {
            name
        } else if !self.description.is_empty() {
            self.description.clone()
        } else {
            self.name.clone()
        }
    }

    /// "BOE NE160QDM-NY1 · 16″", or "Dell Inc. · 27″ · DisplayPort".
    fn details(&self) -> String {
        let mut parts = Vec::new();
        let name = if self.internal() { format!("{} {}", self.make, self.model) } else { self.make.clone() };
        if !name.trim().is_empty() {
            parts.push(name.trim().to_owned());
        }
        let (width, height) = self.physical;
        if width > 0 && height > 0 {
            parts.push(format!("{:.0}″", f64::from(width).hypot(f64::from(height)) / 25.4));
        }
        let connector = self.name.split('-').next().unwrap_or_default();
        let kind = match connector {
            "DP" => "DisplayPort",
            "HDMI" => "HDMI",
            "DVI" => "DVI",
            "VGA" => "VGA",
            _ => "",
        };
        if !kind.is_empty() {
            parts.push(kind.into());
        }
        parts.join(" · ")
    }

    /// Size on the desktop in logical pixels, as it is now.
    fn logical(&self) -> (i32, i32) {
        let Some(mode) = &self.mode else { return (0, 0) };
        let (width, height) = if self.transform.ends_with("90") || self.transform.ends_with("270") { (mode.height, mode.width) } else { (mode.width, mode.height) };
        let scale = self.scale.max(0.25);
        ((f64::from(width) / scale).round() as i32, (f64::from(height) / scale).round() as i32)
    }

    fn output(&self) -> Output {
        Output { id: self.name.clone(), enabled: self.enabled, mode: self.mode.as_ref().map(Mode::text), position: (self.x, self.y), scale: self.scale, transform: self.transform.clone() }
    }
}

/// What `Apply` takes for one monitor.
#[derive(Serialize)]
struct Output {
    id: String,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    position: (i32, i32),
    scale: f64,
    transform: String,
}

const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];
const ROTATIONS: [(&str, &str); 4] = [("normal", "Standard"), ("90", "90°"), ("180", "180°"), ("270", "270°")];

/// A monitor in the arrangement: where it's drawn, in pixels.
struct Tile {
    head: usize,
    widget: gtk::Box,
    rect: (f64, f64, f64, f64),
}

struct Display {
    heads: RefCell<Vec<Head>>,
    selected: RefCell<String>,
    proxy: RefCell<Option<DisplaysProxy<'static>>>,
    area: gtk::DrawingArea,
    fixed: gtk::Fixed,
    tiles: RefCell<Vec<Tile>>,
    /// Pixels per logical pixel, and where logical (0, 0) is drawn.
    view: Cell<(f64, f64, f64)>,
    monitor: gtk::Box,
    status: gtk::Label,
    backlight: Option<Backlight>,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Display", None));

    let area = gtk::DrawingArea::builder().hexpand(true).content_height(150).build();
    let fixed = gtk::Fixed::new();
    let overlay = gtk::Overlay::builder().child(&area).build();
    overlay.add_overlay(&fixed);
    overlay.add_css_class("arrangement");
    overlay.set_overflow(gtk::Overflow::Hidden);
    let hint = ui::note("Drag displays to arrange them");
    hint.set_halign(gtk::Align::End);
    hint.set_valign(gtk::Align::End);
    hint.set_margin_end(12);
    hint.set_margin_bottom(8);
    hint.set_can_target(false);
    overlay.add_overlay(&hint);
    column.append(&overlay);

    let monitor = gtk::Box::new(gtk::Orientation::Vertical, 10);
    column.append(&monitor);
    let status = ui::note("");
    status.set_wrap(true);
    status.set_visible(false);
    column.append(&status);

    let display = Rc::new(Display {
        heads: RefCell::default(),
        selected: RefCell::default(),
        proxy: RefCell::default(),
        area: area.clone(),
        fixed,
        tiles: RefCell::default(),
        view: Cell::new((1.0, 0.0, 0.0)),
        monitor,
        status,
        backlight: Backlight::find(),
    });
    area.connect_resize({
        let display = Rc::downgrade(&display);
        move |_, _, _| {
            if let Some(display) = display.upgrade() {
                display.place();
            }
        }
    });
    display.drag(&overlay);
    glib::spawn_future_local(watch(display.clone()));

    column.append(&super::night::build());
    page.upcast()
}

/// Lists the monitors, then follows `Changed`.
async fn watch(display: Rc<Display>) {
    let connected = async {
        let connection = bus::session().await?;
        let proxy = DisplaysProxy::new(&connection).await?;
        let changes = proxy.receive_changed().await?;
        let json = proxy.list().await?;
        Ok::<_, zbus::Error>((proxy, changes, json))
    };
    let (proxy, mut changes, json) = match connected.await {
        Ok(connected) => connected,
        Err(error) => {
            display.fail(&bus::message(error, "The displays daemon (autark-displays)"));
            return;
        }
    };
    *display.proxy.borrow_mut() = Some(proxy);
    display.update(&json);
    while let Some(signal) = changes.next().await {
        if let Ok(args) = signal.args() {
            display.update(args.heads);
        }
    }
}

impl Display {
    fn fail(&self, message: &str) {
        self.status.set_label(message);
        self.status.add_css_class("error");
        self.status.set_visible(true);
    }

    fn update(self: &Rc<Self>, json: &str) {
        let Ok(heads) = serde_json::from_str::<Vec<Head>>(json) else { return };
        if *self.heads.borrow() == heads && !self.tiles.borrow().is_empty() {
            return;
        }
        {
            let mut selected = self.selected.borrow_mut();
            if !heads.iter().any(|head| head.name == *selected) {
                // The built-in panel first, as it's the one nearest.
                *selected =
                    heads.iter().find(|head| head.internal() && head.enabled).or_else(|| heads.iter().find(|head| head.enabled)).or(heads.first()).map(|head| head.name.clone()).unwrap_or_default();
            }
        }
        *self.heads.borrow_mut() = heads;
        self.status.set_visible(false);
        self.status.remove_css_class("error");
        self.tiles();
        self.rows();
    }

    fn select(self: &Rc<Self>, name: &str) {
        *self.selected.borrow_mut() = name.to_owned();
        self.place();
        self.rows();
    }

    /// One tile per monitor; switched-off ones go after the rest, dimmed.
    fn tiles(self: &Rc<Self>) {
        for tile in self.tiles.borrow_mut().drain(..) {
            self.fixed.remove(&tile.widget);
        }
        let heads = self.heads.borrow();
        let mut tiles = Vec::new();
        for (index, head) in heads.iter().enumerate() {
            let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
            widget.add_css_class("monitor");
            if !head.enabled {
                widget.add_css_class("off");
            }
            widget.set_overflow(gtk::Overflow::Hidden);
            let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            strip.add_css_class("strip");
            widget.append(&strip);
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
            labels.set_vexpand(true);
            labels.set_valign(gtk::Align::Center);
            let short = gtk::Label::new(Some(&if head.internal() { "Built-in".to_owned() } else { head.title() }));
            short.add_css_class("monitor-name");
            short.set_ellipsize(gtk::pango::EllipsizeMode::End);
            labels.append(&short);
            let (width, height) = head.logical();
            let looks = gtk::Label::new(Some(&if head.enabled { format!("{width} × {height}") } else { "Off".into() }));
            looks.add_css_class("monitor-size");
            labels.append(&looks);
            widget.append(&labels);
            self.fixed.put(&widget, 0.0, 0.0);
            tiles.push(Tile { head: index, widget, rect: (0.0, 0.0, 0.0, 0.0) });
        }
        drop(heads);
        *self.tiles.borrow_mut() = tiles;
        self.place();
    }

    /// Lays the tiles out to fit the area: monitors where they are on the desktop, switched-off ones after.
    fn place(&self) {
        let heads = self.heads.borrow();
        let (area_width, area_height) = (f64::from(self.area.width().max(300)), f64::from(self.area.height().max(150)));
        let on: Vec<&Head> = heads.iter().filter(|head| head.enabled && head.mode.is_some()).collect();
        let off = heads.iter().filter(|head| !head.enabled).count();
        let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for head in &on {
            let (width, height) = head.logical();
            (left, top) = (left.min(head.x), top.min(head.y));
            (right, bottom) = (right.max(head.x + width), bottom.max(head.y + height));
        }
        if on.is_empty() {
            (left, top, right, bottom) = (0, 0, 1920, 1080);
        }
        // Room for each switched-off monitor as a small 16:10 tile.
        let off_width = 1920 * off as i32;
        let (total_width, total_height) = (f64::from(right - left + off_width), f64::from(bottom - top));
        let k = ((area_width - 60.0) / total_width).min((area_height - 34.0) / total_height);
        let origin_x = (area_width - total_width * k) / 2.0 - f64::from(left) * k;
        let origin_y = (area_height - total_height * k) / 2.0 - f64::from(top) * k;
        self.view.set((k, origin_x, origin_y));
        let mut next_off = f64::from(right) * k + origin_x + 8.0;
        let selected = self.selected.borrow();
        for tile in self.tiles.borrow_mut().iter_mut() {
            let head = &heads[tile.head];
            let rect = if head.enabled && head.mode.is_some() {
                let (width, height) = head.logical();
                (origin_x + f64::from(head.x) * k + 2.0, origin_y + f64::from(head.y) * k + 2.0, f64::from(width) * k - 4.0, f64::from(height) * k - 4.0)
            } else {
                let rect = (next_off, origin_y + f64::from(top) * k, 1800.0 * k, 1125.0 * k);
                next_off += 1920.0 * k;
                rect
            };
            tile.rect = rect;
            // A short tile drops its size, then its name, so it isn't stretched past where it belongs.
            let labels = tile.widget.last_child();
            if let Some(size) = labels.as_ref().and_then(|labels| labels.last_child()) {
                size.set_visible(rect.3 >= 44.0);
            }
            if let Some(labels) = labels {
                labels.set_visible(rect.3 >= 28.0);
            }
            tile.widget.set_size_request(rect.2.max(24.0) as i32, rect.3.max(12.0) as i32);
            self.fixed.move_(&tile.widget, rect.0, rect.1);
            if head.name == *selected {
                tile.widget.add_css_class("selected");
            } else {
                tile.widget.remove_css_class("selected");
            }
        }
    }

    /// A press selects the monitor under it; dragging one moves it, and dropping it snaps it to the others'
    /// edges. The gesture is on the arrangement, which stays put, so its offsets don't drift as the tile moves.
    fn drag(self: &Rc<Self>, arrangement: &gtk::Overlay) {
        let gesture = gtk::GestureDrag::new();
        let pressed: Rc<Cell<Option<usize>>> = Rc::default();
        gesture.connect_drag_begin({
            let (display, pressed) = (Rc::downgrade(self), pressed.clone());
            move |_, x, y| {
                let Some(display) = display.upgrade() else { return };
                let tiles = display.tiles.borrow();
                let hit = tiles.iter().find(|tile| x >= tile.rect.0 && x <= tile.rect.0 + tile.rect.2 && y >= tile.rect.1 && y <= tile.rect.1 + tile.rect.3);
                pressed.set(hit.map(|tile| tile.head));
            }
        });
        gesture.connect_drag_update({
            let (display, pressed) = (Rc::downgrade(self), pressed.clone());
            move |gesture, dx, dy| {
                let (Some(display), Some(index)) = (display.upgrade(), pressed.get()) else { return };
                if !display.movable(index) || dx.hypot(dy) <= 3.0 {
                    return;
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let tiles = display.tiles.borrow();
                if let Some(tile) = tiles.iter().find(|tile| tile.head == index) {
                    display.fixed.move_(&tile.widget, tile.rect.0 + dx, tile.rect.1 + dy);
                }
            }
        });
        gesture.connect_drag_end({
            let display = Rc::downgrade(self);
            move |_, dx, dy| {
                let (Some(display), Some(index)) = (display.upgrade(), pressed.take()) else { return };
                let name = display.heads.borrow().get(index).map(|head| head.name.clone()).unwrap_or_default();
                display.select(&name);
                if display.movable(index) && dx.hypot(dy) > 3.0 {
                    display.dropped(index, dx, dy);
                }
            }
        });
        arrangement.add_controller(gesture);
    }

    /// Whether a monitor can be dragged: it's on, and there's another on to put it against.
    fn movable(&self, index: usize) -> bool {
        let heads = self.heads.borrow();
        heads.get(index).is_some_and(|head| head.enabled) && heads.iter().filter(|head| head.enabled).count() >= 2
    }

    fn dropped(self: &Rc<Self>, index: usize, dx: f64, dy: f64) {
        let (k, _, _) = self.view.get();
        let mut heads = self.heads.borrow().clone();
        let size = heads[index].logical();
        let wanted = (heads[index].x + (dx / k).round() as i32, heads[index].y + (dy / k).round() as i32);
        let others: Vec<(i32, i32, i32, i32)> = heads
            .iter()
            .enumerate()
            .filter(|(other, head)| *other != index && head.enabled && head.mode.is_some())
            .map(|(_, head)| {
                let (width, height) = head.logical();
                (head.x, head.y, width, height)
            })
            .collect();
        let (x, y) = snap(size, wanted, &others);
        (heads[index].x, heads[index].y) = (x, y);
        // The desktop starts at 0, 0.
        let enabled = heads.iter().filter(|head| head.enabled);
        let (left, top) = enabled.fold((i32::MAX, i32::MAX), |(left, top), head| (left.min(head.x), top.min(head.y)));
        for head in heads.iter_mut().filter(|head| head.enabled) {
            (head.x, head.y) = (head.x - left, head.y - top);
        }
        let outputs: Vec<Output> = heads.iter().filter(|head| head.enabled).map(Head::output).collect();
        self.apply(outputs);
    }

    /// Sends outputs to the daemon; the tiles follow its `Changed`, or snap back on a refusal.
    fn apply(self: &Rc<Self>, outputs: Vec<Output>) {
        let Some(proxy) = self.proxy.borrow().clone() else { return };
        let display = self.clone();
        glib::spawn_future_local(async move {
            let json = serde_json::to_string(&outputs).unwrap_or_default();
            match proxy.apply(&json).await {
                Ok(()) => display.status.set_visible(false),
                Err(error) => {
                    display.fail(&bus::message(error, "The displays daemon (autark-displays)"));
                    display.place();
                    display.rows();
                }
            }
        });
    }

    /// Changes the chosen monitor and applies it.
    fn change(self: &Rc<Self>, change: impl FnOnce(&mut Head)) {
        let selected = self.selected.borrow().clone();
        let Some(mut head) = self.heads.borrow().iter().find(|head| head.name == selected).cloned() else { return };
        change(&mut head);
        self.apply(vec![head.output()]);
    }

    /// The rows for the chosen monitor.
    fn rows(self: &Rc<Self>) {
        ui::clear(&self.monitor);
        let heads = self.heads.borrow();
        let selected = self.selected.borrow().clone();
        let Some(head) = heads.iter().find(|head| head.name == selected).cloned() else { return };
        let several = heads.len() > 1;
        drop(heads);
        let group = ui::group();
        let aside = gtk::Label::new(Some(&head.details()));
        self.monitor.append(&ui::section(&head.title(), Some(&aside), &group));

        if several {
            let display = self.clone();
            let use_it = ui::switch(head.enabled, move |on| display.switched(on));
            group.append(&ui::row("Use this display", None, &use_it));
        }
        let Some(mode) = head.mode.clone().filter(|_| head.enabled) else { return };

        // Resolutions, largest first, and the refresh rates of the one in use.
        let mut sizes: Vec<(i32, i32)> = Vec::new();
        for offered in &head.modes {
            if !sizes.contains(&(offered.width, offered.height)) {
                sizes.push((offered.width, offered.height));
            }
        }
        sizes.sort_by_key(|(width, height)| std::cmp::Reverse((width * height, *width)));
        let values: Vec<String> = sizes.iter().map(|(width, height)| format!("{width}x{height}")).collect();
        let labels: Vec<String> = sizes.iter().map(|(width, height)| format!("{width} × {height}")).collect();
        let options: Vec<(&str, &str)> = values.iter().map(String::as_str).zip(labels.iter().map(String::as_str)).collect();
        let modes = head.modes.clone();
        let refresh = mode.refresh;
        let display = self.clone();
        let resolution = ui::choice(&options, &format!("{}x{}", mode.width, mode.height), move |value| {
            let Some((width, height)) = value.split_once('x').and_then(|(width, height)| Some((width.parse::<i32>().ok()?, height.parse::<i32>().ok()?))) else { return };
            // Keep the refresh rate where the new size has it, else take its fastest.
            let same: Vec<&Mode> = modes.iter().filter(|offered| offered.width == width && offered.height == height).collect();
            let Some(chosen) = same.iter().find(|offered| (offered.refresh - refresh).abs() <= 500).or_else(|| same.iter().max_by_key(|offered| offered.refresh)) else { return };
            let chosen = (*chosen).clone();
            display.change(|head| head.mode = Some(chosen));
        });
        group.append(&ui::row("Resolution", None, &resolution));

        let mut rates: Vec<Mode> = head.modes.iter().filter(|offered| offered.width == mode.width && offered.height == mode.height).cloned().collect();
        rates.sort_by_key(|offered| std::cmp::Reverse(offered.refresh));
        rates.dedup_by_key(|offered| offered.refresh);
        if rates.len() > 1 {
            let values: Vec<String> = rates.iter().map(|rate| rate.refresh.to_string()).collect();
            let labels: Vec<String> = rates.iter().map(Mode::hertz).collect();
            let options: Vec<(&str, &str)> = values.iter().map(String::as_str).zip(labels.iter().map(String::as_str)).collect();
            let current = rates.iter().min_by_key(|rate| (rate.refresh - mode.refresh).abs()).map(|rate| rate.refresh.to_string()).unwrap_or_default();
            let display = self.clone();
            let refresh = ui::choice(&options, &current, move |value| {
                let Some(chosen) = rates.iter().find(|rate| rate.refresh.to_string() == value).cloned() else { return };
                display.change(|head| head.mode = Some(chosen));
            });
            group.append(&ui::row("Refresh rate", None, &refresh));
        }

        let mut scales: Vec<f64> = SCALES.to_vec();
        if !scales.iter().any(|scale| (scale - head.scale).abs() < 0.005) {
            scales.push(head.scale);
            scales.sort_by(f64::total_cmp);
        }
        let values: Vec<String> = scales.iter().map(|scale| format!("{scale}")).collect();
        let labels: Vec<String> = scales.iter().map(|scale| format!("{:.0}%", scale * 100.0)).collect();
        let options: Vec<(&str, &str)> = values.iter().map(String::as_str).zip(labels.iter().map(String::as_str)).collect();
        let current = scales.iter().find(|scale| (*scale - head.scale).abs() < 0.005).map(|scale| format!("{scale}")).unwrap_or_default();
        let display = self.clone();
        let scale = ui::segmented(&options, &current, move |value| {
            if let Ok(scale) = value.parse::<f64>() {
                display.change(|head| head.scale = scale);
            }
        });
        let (width, height) = head.logical();
        let looks = format!("Looks like {width} × {height}");
        group.append(&ui::row("Scale", Some(&looks), &scale));

        if head.internal() {
            if let Some(backlight) = &self.backlight {
                let backlight = backlight.clone();
                let slider = ui::slider(5.0, 100.0, 1.0, backlight.percent(), "☼", "☀", move |percent| backlight.set(percent));
                group.append(&ui::row("Brightness", None, &slider));
            }
        } else {
            let display = self.clone();
            let rotation = ui::choice(&ROTATIONS, &head.transform, move |value| {
                let value = value.to_owned();
                display.change(|head| head.transform = value);
            });
            group.append(&ui::row("Rotation", None, &rotation));
        }
    }

    /// Turns the chosen monitor on, to the right of the others at its preferred mode, or off.
    fn switched(self: &Rc<Self>, on: bool) {
        let right = self.heads.borrow().iter().filter(|head| head.enabled).map(|head| head.x + head.logical().0).max().unwrap_or(0);
        self.change(|head| {
            head.enabled = on;
            if on {
                head.mode = head.modes.iter().find(|mode| mode.preferred).or(head.modes.first()).cloned();
                (head.x, head.y) = (right, 0);
                if head.scale <= 0.0 {
                    head.scale = 1.0;
                }
            }
        });
    }
}

/// Where a monitor of `size` dropped at `wanted` goes: against the nearest edge of another, sharing some of
/// it, without covering any; lined up with that monitor's top, bottom or middle when close.
fn snap(size: (i32, i32), wanted: (i32, i32), others: &[(i32, i32, i32, i32)]) -> (i32, i32) {
    let (width, height) = size;
    let overlaps = |(x, y): (i32, i32)| others.iter().any(|&(ox, oy, ow, oh)| x < ox + ow && ox < x + width && y < oy + oh && oy < y + height);
    let mut candidates = Vec::new();
    for &(ox, oy, ow, oh) in others {
        let free_y = wanted.1.clamp(oy - height + 1, oy + oh - 1);
        let free_x = wanted.0.clamp(ox - width + 1, ox + ow - 1);
        let align = |free: i32, start: i32, length: i32, own: i32| {
            let threshold = (own.min(length) / 10).max(1);
            [start, start + length - own, start + (length - own) / 2].into_iter().filter(|line| (free - line).abs() <= threshold).min_by_key(|line| (free - line).abs()).unwrap_or(free)
        };
        let y = align(free_y, oy, oh, height);
        let x = align(free_x, ox, ow, width);
        candidates.extend([(ox + ow, y), (ox - width, y), (x, oy + oh), (x, oy - height)]);
    }
    let distance = |(x, y): (i32, i32)| (i64::from(x - wanted.0)).pow(2) + (i64::from(y - wanted.1)).pow(2);
    candidates.into_iter().filter(|candidate| !overlaps(*candidate)).min_by_key(|candidate| distance(*candidate)).unwrap_or(wanted)
}

/// The laptop's backlight, set through logind.
#[derive(Clone)]
struct Backlight(autark_common::backlight::Backlight);

impl Backlight {
    fn find() -> Option<Self> {
        autark_common::backlight::Backlight::find().map(Self)
    }

    fn percent(&self) -> f64 {
        self.0.percent()
    }

    fn set(&self, percent: f64) {
        let value = self.0.value(percent);
        let name = self.0.name.clone();
        glib::spawn_future_local(async move {
            let set = async {
                let connection = bus::system().await?;
                SessionProxy::new(&connection).await?.set_brightness("backlight", &name, value).await
            };
            if let Err(error) = set.await {
                eprintln!("settings: brightness: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_monitors_snap_to_an_edge() {
        let laptop = (0, 0, 1707, 1067);
        // Dropped overlapping its right half: goes to the right, tops lined up.
        assert_eq!(snap((2560, 1440), (1200, 30), &[laptop]), (1707, 0));
        // Dropped above: sits on top, centred when nearly so.
        assert_eq!(snap((2560, 1440), (-400, -1300), &[laptop]), (-426, -1440));
        // Bottoms lined up when close.
        assert_eq!(snap((1920, 1080), (-1900, -12), &[laptop]), (-1920, -13));
    }
}
