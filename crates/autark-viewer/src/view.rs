//! The viewer's window: the picture on a quiet ground with a slim bar above it. Scroll zooms toward the pointer,
//! dragging pans, a double click switches between fitting the window and actual size, ← and → (or Page Up and
//! Page Down) go through the folder, R turns it clockwise, F goes full screen, Ctrl+C copies it, Ctrl+O opens
//! another and a file dropped on the window opens too.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib, pango};

const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 64.0;
/// How long the bar stays once the pointer stops, in full screen.
const IDLE: Duration = Duration::from_millis(2500);

/// One loaded picture, turned the right way up by its own orientation tag.
struct Picture {
    pixbuf: Pixbuf,
    surface: cairo::ImageSurface,
}

#[derive(Default)]
struct State {
    /// The pictures beside this one, by name, and which this is.
    files: Vec<PathBuf>,
    index: usize,
    picture: Option<Picture>,
    /// Fitting the window (without growing a small picture), or at `zoom`.
    fit: bool,
    zoom: f64,
    /// Where the picture's centre is, from the window's centre.
    offset: (f64, f64),
    /// Quarter turns clockwise.
    turns: u32,
}

pub struct Viewer {
    app: gtk::Application,
    pub window: gtk::ApplicationWindow,
    canvas: gtk::DrawingArea,
    top: gtk::Box,
    title: gtk::Label,
    info: gtk::Label,
    empty: gtk::Box,
    problem: gtk::Label,
    fullscreen: gtk::Button,
    state: RefCell<State>,
    pointer: Cell<(f64, f64)>,
    /// Bumped each time the bar is shown, so a hide timer from before knows it's out of date.
    shown: Rc<Cell<u32>>,
}

impl State {
    /// The picture's size as shown, after the turns.
    fn shown_size(&self) -> Option<(f64, f64)> {
        let picture = self.picture.as_ref()?;
        let (w, h) = (f64::from(picture.pixbuf.width()), f64::from(picture.pixbuf.height()));
        Some(if self.turns % 2 == 1 { (h, w) } else { (w, h) })
    }

    /// Pixels on screen for each picture pixel.
    fn scale(&self, width: f64, height: f64) -> f64 {
        match self.shown_size() {
            Some((w, h)) if self.fit => (width / w).min(height / h).min(1.0),
            _ => self.zoom,
        }
    }
}

/// `1536` as "1.5 KB".
fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// Whether `path` has an extension one of the loaders reads.
fn readable(path: &Path, extensions: &[String]) -> bool {
    path.extension().is_some_and(|ext| extensions.iter().any(|known| known.eq_ignore_ascii_case(&ext.to_string_lossy())))
}

/// The pictures in `path`'s folder, by name with numbers in order ("2.png" before "10.png").
fn siblings(path: &Path) -> Vec<PathBuf> {
    let extensions: Vec<String> = Pixbuf::formats().iter().flat_map(|format| format.extensions()).map(|ext| ext.to_string()).collect();
    let Some(folder) = path.parent() else { return vec![path.to_owned()] };
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|file| file.is_file() && readable(file, &extensions)).collect();
    files.sort_by_key(|file| natural_key(&file.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default()));
    if !files.iter().any(|file| file == path) {
        files.push(path.to_owned());
    }
    files
}

/// A name split into text and number parts, so numbers compare as numbers.
fn natural_key(name: &str) -> Vec<(String, u64)> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut digits = String::new();
    for c in name.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if !digits.is_empty() {
            parts.push((std::mem::take(&mut text), digits.parse().unwrap_or(u64::MAX)));
            digits.clear();
        }
        text.push(c);
    }
    parts.push((text, 0));
    parts
}

/// A pixbuf as a cairo surface: premultiplied, with blue first in each little-endian pixel.
fn surface_from(pixbuf: &Pixbuf) -> Option<cairo::ImageSurface> {
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let (channels, row, alpha) = (pixbuf.n_channels() as usize, pixbuf.rowstride() as usize, pixbuf.has_alpha());
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).ok()?;
    let stride = surface.stride() as usize;
    let source = pixbuf.read_pixel_bytes();
    {
        let mut data = surface.data().ok()?;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let pixel = source.get(y * row + x * channels..y * row + x * channels + channels)?;
                let a = if alpha { u32::from(pixel[3]) } else { 255 };
                let premultiply = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
                let out = y * stride + x * 4;
                data[out..out + 4].copy_from_slice(&[premultiply(pixel[2]), premultiply(pixel[1]), premultiply(pixel[0]), a as u8]);
            }
        }
    }
    Some(surface)
}

/// Two squares by two of the theme's greys, for behind transparency.
fn checkerboard(palette: &autark_common::theme::Palette) -> Option<cairo::ImageSurface> {
    let tile = cairo::ImageSurface::create(cairo::Format::ARgb32, 16, 16).ok()?;
    let cr = cairo::Context::new(&tile).ok()?;
    autark_common::theme::set_source(&cr, &palette.surface, 1.0);
    cr.paint().ok()?;
    autark_common::theme::set_source(&cr, &palette.text, 0.1);
    cr.rectangle(0.0, 0.0, 8.0, 8.0);
    cr.rectangle(8.0, 8.0, 8.0, 8.0);
    cr.fill().ok()?;
    Some(tile)
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("tool");
    button
}

impl Viewer {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Viewer").default_width(1100).default_height(760).build();
        window.add_css_class("viewer");
        // The window's controls sit in the bar, so the titlebar is empty.
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let canvas = gtk::DrawingArea::builder().hexpand(true).vexpand(true).build();
        canvas.add_css_class("canvas");
        canvas.set_cursor_from_name(Some("grab"));

        let controls = gtk::WindowControls::new(gtk::PackType::Start);
        controls.set_decoration_layout(Some("close,minimize,maximize:"));
        let title = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
        title.add_css_class("title");
        let info = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        info.add_css_class("info");
        let previous = icon_button("go-previous-symbolic", "Previous (←)");
        let next = icon_button("go-next-symbolic", "Next (→)");
        let rotate = icon_button("object-rotate-right-symbolic", "Rotate (R)");
        let fit = icon_button("zoom-fit-best-symbolic", "Fit to window (0)");
        let fullscreen = icon_button("view-fullscreen-symbolic", "Full screen (F)");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top.add_css_class("top");
        top.set_valign(gtk::Align::Start);
        for widget in [controls.upcast_ref::<gtk::Widget>(), title.upcast_ref(), info.upcast_ref(), previous.upcast_ref(), next.upcast_ref(), rotate.upcast_ref(), fit.upcast_ref(), fullscreen.upcast_ref()] {
            top.append(widget);
        }

        let open = gtk::Button::with_label("Open a picture");
        open.add_css_class("open");
        open.set_action_name(Some("win.open"));
        let hint = gtk::Label::new(Some("Or drop one here."));
        hint.add_css_class("hint");
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 14);
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        empty.append(&gtk::Image::builder().icon_name("image-x-generic-symbolic").pixel_size(56).css_classes(["glyph"]).build());
        empty.append(&open);
        empty.append(&hint);
        let problem = gtk::Label::builder().wrap(true).max_width_chars(60).justify(gtk::Justification::Center).halign(gtk::Align::Center).valign(gtk::Align::Center).visible(false).build();
        problem.add_css_class("problem");

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&canvas));
        overlay.add_overlay(&empty);
        overlay.add_overlay(&problem);
        overlay.add_overlay(&top);
        window.set_child(Some(&overlay));

        let viewer = Rc::new(Self {
            app: app.clone(),
            window,
            canvas,
            top,
            title,
            info,
            empty,
            problem,
            fullscreen,
            state: RefCell::new(State { fit: true, zoom: 1.0, ..State::default() }),
            pointer: Cell::new((0.0, 0.0)),
            shown: Rc::default(),
        });
        let me = Rc::downgrade(&viewer);
        let click = |button: &gtk::Button, act: fn(&Rc<Viewer>)| {
            let me = me.clone();
            button.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    act(&me);
                }
            });
        };
        click(&previous, |me| me.step(-1));
        click(&next, |me| me.step(1));
        click(&rotate, |me| me.rotate());
        click(&fit, |me| me.fit_window());
        click(&viewer.fullscreen, |me| me.toggle_fullscreen());
        viewer.connect();
        viewer
    }

    fn connect(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        self.canvas.set_draw_func({
            let me = me.clone();
            move |_, cr, width, height| {
                if let Some(me) = me.upgrade() {
                    me.draw(cr, f64::from(width), f64::from(height));
                }
            }
        });
        autark_common::theme::redraw_on_change(&self.canvas);

        let open = gio::SimpleAction::new("open", None);
        open.connect_activate({
            let me = me.clone();
            move |_, _| {
                if let Some(me) = me.upgrade() {
                    me.choose_file();
                }
            }
        });
        self.window.add_action(&open);
        self.app.set_accels_for_action("win.open", &["<Control>o"]);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let me = me.clone();
            move |_, key, _, modifiers| {
                let Some(me) = me.upgrade() else { return glib::Propagation::Proceed };
                let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                me.show_bar();
                match key {
                    gdk::Key::Right | gdk::Key::Page_Down | gdk::Key::space => me.step(1),
                    gdk::Key::Left | gdk::Key::Page_Up | gdk::Key::BackSpace => me.step(-1),
                    gdk::Key::Home => me.go(0),
                    gdk::Key::End => me.go(usize::MAX),
                    gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => me.zoom_by(1.25, None),
                    gdk::Key::minus | gdk::Key::KP_Subtract => me.zoom_by(0.8, None),
                    gdk::Key::_0 | gdk::Key::KP_0 => me.fit_window(),
                    gdk::Key::_1 | gdk::Key::KP_1 => me.actual_size(None),
                    gdk::Key::r | gdk::Key::R if !control => me.turn(if shift { 3 } else { 1 }),
                    gdk::Key::f | gdk::Key::F | gdk::Key::F11 => me.toggle_fullscreen(),
                    gdk::Key::Escape if me.window.is_fullscreen() => me.toggle_fullscreen(),
                    gdk::Key::c if control => me.copy(),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        self.window.add_controller(keys);

        // Scrolling zooms toward the pointer, so what you point at stays under it.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll({
            let me = me.clone();
            move |_, _, dy| {
                if let Some(me) = me.upgrade() {
                    me.zoom_by(1.12f64.powf(-dy), Some(me.pointer.get()));
                }
                glib::Propagation::Stop
            }
        });
        self.canvas.add_controller(scroll);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let me = me.clone();
            move |_, x, y| {
                if let Some(me) = me.upgrade() {
                    me.pointer.set((x, y));
                    me.show_bar();
                }
            }
        });
        self.window.add_controller(motion);

        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0, 0.0)));
        drag.connect_drag_begin({
            let (me, start) = (me.clone(), start.clone());
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    start.set(me.state.borrow().offset);
                    me.canvas.set_cursor_from_name(Some("grabbing"));
                }
            }
        });
        drag.connect_drag_update({
            let (me, start) = (me.clone(), start.clone());
            move |_, dx, dy| {
                if let Some(me) = me.upgrade() {
                    me.state.borrow_mut().offset = (start.get().0 + dx, start.get().1 + dy);
                    me.canvas.queue_draw();
                }
            }
        });
        drag.connect_drag_end({
            let me = me.clone();
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    me.canvas.set_cursor_from_name(Some("grab"));
                }
            }
        });
        self.canvas.add_controller(drag);

        // A double click switches between fitting the window and actual size.
        let click = gtk::GestureClick::new();
        click.connect_released({
            let me = me.clone();
            move |_, presses, x, y| {
                if let (Some(me), 2) = (me.upgrade(), presses) {
                    let fitting = me.state.borrow().fit;
                    if fitting { me.actual_size(Some((x, y))) } else { me.fit_window() }
                }
            }
        });
        self.canvas.add_controller(click);

        // Pinching on a touchpad or screen.
        let pinch = gtk::GestureZoom::new();
        let base = Rc::new(Cell::new(1.0));
        pinch.connect_begin({
            let (me, base) = (me.clone(), base.clone());
            move |_, _| {
                if let Some(me) = me.upgrade() {
                    base.set(me.current_scale());
                }
            }
        });
        pinch.connect_scale_changed({
            let (me, base) = (me.clone(), base.clone());
            move |_, scale| {
                if let Some(me) = me.upgrade() {
                    let now = me.current_scale();
                    me.zoom_by(base.get() * scale / now, Some(me.pointer.get()));
                }
            }
        });
        self.canvas.add_controller(pinch);

        // The window's handlers only hold the viewer weakly, so something has to keep it alive until the window closes.
        let keep = self.clone();
        self.window.connect_close_request(move |_| {
            let _ = &keep;
            glib::Propagation::Proceed
        });

        let drop = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        drop.connect_drop({
            let me = me.clone();
            move |_, value, _, _| match (me.upgrade(), value.get::<gio::File>().ok().and_then(|file| file.path())) {
                (Some(me), Some(path)) => {
                    me.open(&path);
                    true
                }
                _ => false,
            }
        });
        self.window.add_controller(drop);
    }

    /// Shows `path`, and the pictures beside it to step through.
    pub fn open(self: &Rc<Self>, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        let files = siblings(&path);
        let index = files.iter().position(|file| *file == path).unwrap_or(0);
        {
            let mut state = self.state.borrow_mut();
            state.files = files;
            state.index = index;
        }
        self.load();
    }

    /// Loads the current file, fitted to the window and the right way up.
    fn load(&self) {
        let (path, position) = {
            let state = self.state.borrow();
            (state.files.get(state.index).cloned(), (state.index + 1, state.files.len()))
        };
        let Some(path) = path else { return };
        let loaded = Pixbuf::from_file(&path)
            .map_err(|error| error.to_string())
            .map(|pixbuf| pixbuf.apply_embedded_orientation().unwrap_or(pixbuf))
            .and_then(|pixbuf| {
                surface_from(&pixbuf).map(|surface| Picture { pixbuf, surface }).ok_or_else(|| "the picture couldn't be drawn".to_owned())
            });
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        match loaded {
            Ok(picture) => {
                let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                let mut info = format!("{} × {} · {}", picture.pixbuf.width(), picture.pixbuf.height(), size_text(bytes));
                if position.1 > 1 {
                    info.push_str(&format!(" · {} of {}", position.0, position.1));
                }
                {
                    let mut state = self.state.borrow_mut();
                    state.picture = Some(picture);
                    state.fit = true;
                    state.offset = (0.0, 0.0);
                    state.turns = 0;
                }
                self.title.set_text(&name);
                self.info.set_text(&info);
                self.window.set_title(Some(&name));
                self.empty.set_visible(false);
                self.problem.set_visible(false);
            }
            Err(error) => {
                self.state.borrow_mut().picture = None;
                self.title.set_text(&name);
                self.info.set_text("");
                self.fail(&format!("Couldn’t open {name}: {error}"));
            }
        }
        self.canvas.queue_draw();
    }

    pub fn fail(&self, message: &str) {
        self.empty.set_visible(self.state.borrow().picture.is_none() && self.state.borrow().files.is_empty());
        self.problem.set_text(message);
        self.problem.set_visible(true);
    }

    /// The next picture (`1`) or the previous (`-1`), wrapping round.
    fn step(self: &Rc<Self>, by: isize) {
        let count = self.state.borrow().files.len();
        if count < 2 {
            return;
        }
        let index = (self.state.borrow().index as isize + by).rem_euclid(count as isize) as usize;
        self.go(index);
    }

    fn go(self: &Rc<Self>, index: usize) {
        {
            let mut state = self.state.borrow_mut();
            let Some(last) = state.files.len().checked_sub(1) else { return };
            state.index = index.min(last);
        }
        self.load();
    }

    fn choose_file(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Pictures"));
        filter.add_mime_type("image/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("Open a picture").filters(&filters).build();
        let me = Rc::downgrade(self);
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            if let (Some(me), Ok(file)) = (me.upgrade(), result)
                && let Some(path) = file.path()
            {
                me.open(&path);
            }
        });
    }

    /// Pixels on screen for each picture pixel right now.
    fn current_scale(&self) -> f64 {
        self.state.borrow().scale(f64::from(self.canvas.width()), f64::from(self.canvas.height()))
    }

    /// Zooms by `factor`, keeping the point under `around` (in the window) where it is; without one, the centre.
    fn zoom_by(&self, factor: f64, around: Option<(f64, f64)>) {
        let (width, height) = (f64::from(self.canvas.width()), f64::from(self.canvas.height()));
        let mut state = self.state.borrow_mut();
        if state.picture.is_none() {
            return;
        }
        let now = state.scale(width, height);
        let next = (now * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let (px, py) = around.map_or((0.0, 0.0), |(x, y)| (x - width / 2.0, y - height / 2.0));
        state.offset = (px - (px - state.offset.0) * next / now, py - (py - state.offset.1) * next / now);
        state.zoom = next;
        state.fit = false;
        drop(state);
        self.canvas.queue_draw();
    }

    fn fit_window(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.fit = true;
            state.offset = (0.0, 0.0);
        }
        self.canvas.queue_draw();
    }

    /// One picture pixel to one on screen, around `around` or the centre.
    fn actual_size(&self, around: Option<(f64, f64)>) {
        let now = self.current_scale();
        if now > 0.0 {
            self.zoom_by(1.0 / now, around);
        }
    }

    fn rotate(&self) {
        self.turn(1);
    }

    fn turn(&self, quarters: u32) {
        {
            let mut state = self.state.borrow_mut();
            if state.picture.is_none() {
                return;
            }
            state.turns = (state.turns + quarters) % 4;
            state.offset = (0.0, 0.0);
        }
        self.canvas.queue_draw();
    }

    /// Puts the picture, as shown, on the clipboard.
    fn copy(&self) {
        let state = self.state.borrow();
        let Some(picture) = &state.picture else { return };
        let mut pixbuf = picture.pixbuf.clone();
        for _ in 0..state.turns {
            pixbuf = pixbuf.rotate_simple(gtk::gdk_pixbuf::PixbufRotation::Clockwise).unwrap_or(pixbuf);
        }
        self.window.clipboard().set_texture(&gdk::Texture::for_pixbuf(&pixbuf));
    }

    fn toggle_fullscreen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
            self.fullscreen.set_icon_name("view-fullscreen-symbolic");
        } else {
            self.window.fullscreen();
            self.fullscreen.set_icon_name("view-restore-symbolic");
        }
        self.show_bar();
    }

    /// Shows the bar, and in full screen hides it again once the pointer has been still.
    fn show_bar(&self) {
        self.top.remove_css_class("faded");
        self.top.set_can_target(true);
        let generation = self.shown.get().wrapping_add(1);
        self.shown.set(generation);
        if !self.window.is_fullscreen() {
            return;
        }
        let (top, shown) = (self.top.downgrade(), self.shown.clone());
        glib::timeout_add_local_once(IDLE, move || {
            if let Some(top) = top.upgrade()
                && shown.get() == generation
            {
                top.add_css_class("faded");
                top.set_can_target(false);
            }
        });
    }

    fn draw(&self, cr: &cairo::Context, width: f64, height: f64) {
        let look = autark_common::theme::look();
        autark_common::theme::set_source(cr, &look.palette.ground, 1.0);
        let _ = cr.paint();
        let state = self.state.borrow();
        let (Some(picture), Some((w, h))) = (&state.picture, state.shown_size()) else { return };
        let scale = state.scale(width, height);
        let (pw, ph) = (f64::from(picture.pixbuf.width()), f64::from(picture.pixbuf.height()));
        cr.translate((width / 2.0 + state.offset.0).round(), (height / 2.0 + state.offset.1).round());
        cr.rotate(f64::from(state.turns) * std::f64::consts::FRAC_PI_2);
        cr.scale(scale, scale);
        cr.translate(-pw / 2.0, -ph / 2.0);
        if picture.pixbuf.has_alpha() {
            // Transparent parts show a checkerboard, 16 screen pixels to a tile, so they read as empty and not as part of the ground.
            if let Some(tile) = checkerboard(&look.palette) {
                let pattern = cairo::SurfacePattern::create(&tile);
                pattern.set_extend(cairo::Extend::Repeat);
                pattern.set_matrix(cairo::Matrix::new(scale, 0.0, 0.0, scale, 0.0, 0.0));
                let _ = cr.set_source(&pattern);
                cr.rectangle(0.0, 0.0, pw, ph);
                let _ = cr.fill();
            }
        }
        if cr.set_source_surface(&picture.surface, 0.0, 0.0).is_ok() {
            // Enlarged far enough, pixels stay crisp; shrunk or slightly enlarged, they're smoothed.
            cairo::SurfacePattern::try_from(cr.source()).ok().inspect(|pattern| pattern.set_filter(if scale >= 3.0 { cairo::Filter::Nearest } else { cairo::Filter::Good }));
            cr.rectangle(0.0, 0.0, pw, ph);
            let _ = cr.fill();
        }
        // A thin edge, so a picture as pale as the ground still has one.
        autark_common::theme::set_source(cr, &look.palette.text, 0.14);
        cr.set_line_width(1.0 / scale);
        cr.rectangle(-0.5 / scale, -0.5 / scale, pw + 1.0 / scale, ph + 1.0 / scale);
        let _ = cr.stroke();
        let _ = (w, h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_a_file_manager_shows_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn numbers_in_names_sort_as_numbers() {
        let mut names = vec!["img10.png", "img2.png", "Img1.png", "b.png", "a.png"];
        names.sort_by_key(|name| natural_key(&name.to_lowercase()));
        assert_eq!(names, ["a.png", "b.png", "Img1.png", "img2.png", "img10.png"]);
    }

    #[test]
    fn fitting_never_grows_a_small_picture() {
        let state = State { fit: true, zoom: 1.0, ..State::default() };
        assert_eq!(state.scale(800.0, 600.0), 1.0);
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;

    #[test]
    fn a_pixbuf_becomes_the_same_picture_as_a_surface() {
        let pixbuf = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 2, 1).unwrap();
        // Opaque red, then half-transparent green.
        pixbuf.put_pixel(0, 0, 255, 0, 0, 255);
        pixbuf.put_pixel(1, 0, 0, 200, 0, 128);
        let mut surface = surface_from(&pixbuf).unwrap();
        let data = surface.data().unwrap();
        assert_eq!(data[0..4], [0, 0, 255, 255], "opaque red as BGRA");
        assert_eq!(data[4..8], [0, 100, 0, 128], "half-transparent green, premultiplied");
    }
}
