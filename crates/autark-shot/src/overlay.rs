//! Choosing what to capture, over a frozen copy of the screen: one layer-shell surface per output showing its
//! capture, dimmed outside the choice. Drag for an area; Space switches to picking a window; Escape leaves.
//! With the options bar (Shift+Super+5) the choice stays until Capture, and the bar switches modes.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::capture::Shot;
use crate::options::{Options, SaveTo};
use crate::windows::Window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Screen,
    Window,
    Area,
}

/// What was chosen. Rectangles are logical pixels relative to the output.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Screen(usize),
    Area(usize, i32, i32, i32, i32),
    Cancel,
}

struct State {
    mode: Mode,
    /// Output index and two corners, while dragging or once dragged.
    area: Option<(usize, f64, f64, f64, f64)>,
    dragging: bool,
    pointer: Option<(usize, f64, f64)>,
    windows: Vec<Window>,
    options: Options,
    bar: bool,
    done: Option<Box<dyn FnOnce(Choice, Options)>>,
    surfaces: Vec<(gtk::ApplicationWindow, gtk::DrawingArea)>,
}

pub fn choose(app: &gtk::Application, shots: Rc<Vec<Shot>>, windows: Vec<Window>, options: Options, bar: bool, done: impl FnOnce(Choice, Options) + 'static) {
    let area = options.area.as_ref().and_then(|(name, x, y, w, h)| {
        let index = shots.iter().position(|shot| shot.name == *name)?;
        Some((index, f64::from(*x), f64::from(*y), f64::from(x + w), f64::from(y + h)))
    });
    let state =
        Rc::new(RefCell::new(State { mode: Mode::Area, area: if bar { area } else { None }, dragging: false, pointer: None, windows, options, bar, done: Some(Box::new(done)), surfaces: Vec::new() }));
    let monitors = gdk::Display::default().map(|display| display.monitors()).into_iter().flat_map(|list| list.iter::<gdk::Monitor>().flatten().collect::<Vec<_>>()).collect::<Vec<_>>();
    let bar_output = state.borrow().area.map_or(0, |(index, ..)| index);
    for (index, shot) in shots.iter().enumerate() {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("shot-overlay");
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("autark-shot"));
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window.set_exclusive_zone(-1);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        if let Some(monitor) = monitors.iter().find(|monitor| monitor.connector().is_some_and(|name| name == shot.name.as_str())) {
            window.set_monitor(Some(monitor));
        }
        let picture = gtk::Picture::for_paintable(&crate::texture(&shot.image));
        picture.set_content_fit(gtk::ContentFit::Fill);
        picture.set_can_shrink(true);
        let area = gtk::DrawingArea::new();
        area.set_cursor_from_name(Some("crosshair"));
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&area);
        if bar && index == bar_output {
            overlay.add_overlay(&options_bar(&state, &shots));
        }
        window.set_child(Some(&overlay));

        // The one strong hold on the state; `finish` lets go of it.
        let (draw_state, draw_shots) = (state.clone(), shots.clone());
        area.set_draw_func(move |_, cr, width, height| draw(cr, &draw_state.borrow(), index, &draw_shots[index], f64::from(width), f64::from(height)));
        input(&state, &shots, index, &window, &area);
        state.borrow_mut().surfaces.push((window.clone(), area));
        window.present();
    }
}

fn redraw(state: &State) {
    for (_, area) in &state.surfaces {
        area.queue_draw();
    }
}

fn finish(state: &Rc<RefCell<State>>, choice: Choice) {
    let (done, surfaces, options) = {
        let mut state = state.borrow_mut();
        (state.done.take(), std::mem::take(&mut state.surfaces), state.options.clone())
    };
    for (window, area) in surfaces {
        area.set_draw_func(|_, _, _, _| {});
        window.close();
    }
    if let Some(done) = done {
        done(choice, options);
    }
}

/// The current choice, ready to capture.
fn current(state: &State, shots: &[Shot]) -> Option<Choice> {
    match state.mode {
        Mode::Screen => Some(Choice::Screen(state.pointer.map_or(0, |(index, ..)| index))),
        Mode::Window | Mode::Area => {
            let (index, x0, y0, x1, y1) = state.area?;
            let (x, y) = (x0.min(x1).round() as i32, y0.min(y1).round() as i32);
            let (w, h) = ((x1 - x0).abs().round() as i32, (y1 - y0).abs().round() as i32);
            let shot = shots.get(index)?;
            (w > 1 && h > 1).then(|| Choice::Area(index, x.max(0), y.max(0), w.min(shot.width - x.max(0)), h.min(shot.height - y.max(0))))
        }
    }
}

/// The window under a point on an output, as an area on that output.
fn window_at(state: &State, shot: &Shot, index: usize, x: f64, y: f64) -> Option<(usize, f64, f64, f64, f64)> {
    let (gx, gy) = (x + f64::from(shot.x), y + f64::from(shot.y));
    let window = state.windows.iter().find(|w| gx >= f64::from(w.x) && gx < f64::from(w.x + w.width) && gy >= f64::from(w.y) && gy < f64::from(w.y + w.height))?;
    let (x0, y0) = (f64::from(window.x - shot.x).max(0.0), f64::from(window.y - shot.y).max(0.0));
    let (x1, y1) = (f64::from(window.x + window.width - shot.x).min(f64::from(shot.width)), f64::from(window.y + window.height - shot.y).min(f64::from(shot.height)));
    Some((index, x0, y0, x1, y1))
}

fn input(state: &Rc<RefCell<State>>, shots: &Rc<Vec<Shot>>, index: usize, window: &gtk::ApplicationWindow, area: &gtk::DrawingArea) {
    let motion = gtk::EventControllerMotion::new();
    let (weak, shots_for_motion) = (Rc::downgrade(state), shots.clone());
    motion.connect_motion(move |_, x, y| {
        let Some(state) = weak.upgrade() else { return };
        let mut st = state.borrow_mut();
        st.pointer = Some((index, x, y));
        if st.mode == Mode::Window {
            st.area = window_at(&st, &shots_for_motion[index], index, x, y);
        }
        redraw(&st);
    });
    area.add_controller(motion);

    let drag = gtk::GestureDrag::new();
    let weak = Rc::downgrade(state);
    drag.connect_drag_begin(move |_, x, y| {
        let Some(state) = weak.upgrade() else { return };
        let mut st = state.borrow_mut();
        if st.mode == Mode::Area {
            st.area = Some((index, x, y, x, y));
            st.dragging = true;
        }
        redraw(&st);
    });
    let weak = Rc::downgrade(state);
    drag.connect_drag_update(move |gesture, dx, dy| {
        let Some(state) = weak.upgrade() else { return };
        let mut st = state.borrow_mut();
        if st.dragging
            && let (Some((i, x0, y0, _, _)), Some((sx, sy))) = (st.area, gesture.start_point())
        {
            st.area = Some((i, x0, y0, sx + dx, sy + dy));
            st.pointer = Some((index, sx + dx, sy + dy));
        }
        redraw(&st);
    });
    let (weak, shots_for_end) = (Rc::downgrade(state), shots.clone());
    drag.connect_drag_end(move |_, _, _| {
        let Some(state) = weak.upgrade() else { return };
        let choice = {
            let mut st = state.borrow_mut();
            st.dragging = false;
            redraw(&st);
            // Without the bar a release captures, as on a Mac; a click without a drag is nothing.
            if st.bar { None } else { current(&st, &shots_for_end) }
        };
        if let Some(choice) = choice {
            finish(&state, choice);
        }
    });
    area.add_controller(drag);

    let keys = gtk::EventControllerKey::new();
    let (weak, shots_for_keys) = (Rc::downgrade(state), shots.clone());
    keys.connect_key_pressed(move |_, key, _, _| {
        let Some(state) = weak.upgrade() else { return glib::Propagation::Proceed };
        match key {
            gdk::Key::Escape => finish(&state, Choice::Cancel),
            gdk::Key::space => {
                let mut st = state.borrow_mut();
                if !st.windows.is_empty() {
                    st.mode = if st.mode == Mode::Window { Mode::Area } else { Mode::Window };
                    st.area = None;
                    if let (Mode::Window, Some((i, x, y))) = (st.mode, st.pointer) {
                        st.area = window_at(&st, &shots_for_keys[i], i, x, y);
                    }
                    let cursor = if st.mode == Mode::Window { "pointer" } else { "crosshair" };
                    for (_, area) in &st.surfaces {
                        area.set_cursor_from_name(Some(cursor));
                    }
                    redraw(&st);
                }
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let choice = current(&state.borrow(), &shots_for_keys);
                if let Some(choice) = choice {
                    finish(&state, choice);
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
}

fn draw(cr: &gtk::cairo::Context, state: &State, index: usize, shot: &Shot, width: f64, height: f64) {
    let (sx, sy) = (width / f64::from(shot.width.max(1)), height / f64::from(shot.height.max(1)));
    let chosen = match (state.mode, state.area) {
        (Mode::Screen, _) => state.pointer.is_none_or(|(i, ..)| i == index).then_some((0.0, 0.0, width, height)),
        (_, Some((i, x0, y0, x1, y1))) if i == index => Some((x0.min(x1) * sx, y0.min(y1) * sy, (x1 - x0).abs() * sx, (y1 - y0).abs() * sy)),
        _ => None,
    };
    // Dim everything but the choice.
    cr.set_source_rgba(36.0 / 255.0, 34.0 / 255.0, 30.0 / 255.0, 0.38);
    cr.rectangle(0.0, 0.0, width, height);
    if let Some((x, y, w, h)) = chosen {
        cr.rectangle(x, y + h, w, -h);
    }
    cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
    let _ = cr.fill();
    if let Some((x, y, w, h)) = chosen
        && state.mode != Mode::Screen
    {
        if state.mode == Mode::Window {
            autark_common::theme::set_source(cr, &autark_common::theme::look().palette.accent, 0.16);
            cr.rectangle(x, y, w, h);
            let _ = cr.fill();
        }
        cr.set_line_width(1.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.rectangle(x.round() + 0.5, y.round() + 0.5, w.round() - 1.0, h.round() - 1.0);
        let _ = cr.stroke();
        if state.mode == Mode::Area && !state.dragging {
            for (hx, hy) in [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (0.0, 0.5), (1.0, 0.5), (0.0, 1.0), (0.5, 1.0), (1.0, 1.0)] {
                cr.arc(x + w * hx, y + h * hy, 4.0, 0.0, std::f64::consts::TAU);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(36.0 / 255.0, 34.0 / 255.0, 30.0 / 255.0, 0.35);
                let _ = cr.stroke();
            }
        }
        let scale = shot.scale();
        label(cr, x, (y - 26.0).max(4.0), &format!("{} × {}", (w / sx * scale).round(), (h / sy * scale).round()));
    } else if let (Mode::Area, Some((i, px, py))) = (state.mode, state.pointer)
        && i == index
    {
        label(cr, px + 14.0, py + 14.0, &format!("{}, {}", px.round(), py.round()));
    }
}

/// A small dark label with monospaced text, its top-left at (x, y).
fn label(cr: &gtk::cairo::Context, x: f64, y: f64, text: &str) {
    cr.select_font_face("Geist Mono", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
    cr.set_font_size(11.0);
    let Ok(extents) = cr.text_extents(text) else { return };
    let (w, h) = (extents.x_advance() + 12.0, 20.0);
    cr.new_sub_path();
    let r = 6.0;
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
    cr.set_source_rgba(36.0 / 255.0, 34.0 / 255.0, 30.0 / 255.0, 0.78);
    let _ = cr.fill();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.move_to(x + 6.0, y + 14.0);
    let _ = cr.show_text(text);
}

/// The Shift+Super+5 bar: close, the three modes, recording (later), Options and Capture.
fn options_bar(state: &Rc<RefCell<State>>, shots: &Rc<Vec<Shot>>) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    bar.add_css_class("shot-bar");
    bar.set_halign(gtk::Align::Center);
    bar.set_valign(gtk::Align::End);
    bar.set_margin_bottom(96);

    let close = gtk::Button::new();
    close.set_child(Some(&glyph(Glyph::Close, false)));
    close.set_tooltip_text(Some("Close (Esc)"));
    let weak = Rc::downgrade(state);
    close.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            finish(&state, Choice::Cancel);
        }
    });
    bar.append(&close);
    bar.append(&separator());

    let mut buttons: Vec<(Mode, gtk::ToggleButton)> = Vec::new();
    let has_windows = !state.borrow().windows.is_empty();
    for (mode, tip) in [(Mode::Screen, "Capture the entire screen"), (Mode::Window, "Capture a window"), (Mode::Area, "Capture an area")] {
        let button = gtk::ToggleButton::new();
        button.add_css_class("mode");
        button.set_tooltip_text(Some(tip));
        button.set_active(mode == Mode::Area);
        button.set_sensitive(mode != Mode::Window || has_windows);
        button.set_child(Some(&glyph(Glyph::Mode(mode), mode == Mode::Area)));
        if let Some((_, first)) = buttons.first() {
            button.set_group(Some(first));
        }
        let weak = Rc::downgrade(state);
        button.connect_toggled(move |button| {
            if let Some(child) = button.child()
                && let Ok(area) = child.downcast::<gtk::DrawingArea>()
            {
                set_glyph(&area, Glyph::Mode(mode), button.is_active());
            }
            let Some(state) = weak.upgrade() else { return };
            if button.is_active() {
                let mut st = state.borrow_mut();
                st.mode = mode;
                if mode != Mode::Area {
                    st.area = None;
                }
                redraw(&st);
            }
        });
        buttons.push((mode, button.clone()));
        bar.append(&button);
    }
    bar.append(&separator());

    let record = gtk::Button::new();
    record.set_child(Some(&glyph(Glyph::Record, false)));
    record.set_sensitive(false);
    record.set_tooltip_text(Some("Recording comes later"));
    bar.append(&record);
    bar.append(&separator());

    let menu = gtk::ToggleButton::new();
    menu.add_css_class("options");
    let label = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    label.append(&gtk::Label::new(Some("Options")));
    label.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
    menu.set_child(Some(&label));
    let popover = options_menu(state);
    popover.add_css_class("shot-popover");
    popover.set_parent(&menu);
    let parented = popover.clone();
    menu.connect_destroy(move |_| parented.unparent());
    popover.set_position(gtk::PositionType::Top);
    let shown = popover.clone();
    menu.connect_toggled(move |menu| if menu.is_active() { shown.popup() } else { shown.popdown() });
    let weak = menu.downgrade();
    popover.connect_closed(move |_| {
        if let Some(menu) = weak.upgrade() {
            menu.set_active(false);
        }
    });
    bar.append(&menu);

    let capture = gtk::Button::with_label("Capture");
    capture.add_css_class("capture");
    let (weak, shots) = (Rc::downgrade(state), shots.clone());
    capture.connect_clicked(move |_| {
        let Some(state) = weak.upgrade() else { return };
        let choice = current(&state.borrow(), &shots);
        if let Some(choice) = choice {
            finish(&state, choice);
        }
    });
    bar.append(&capture);
    bar
}

fn separator() -> gtk::Separator {
    let separator = gtk::Separator::new(gtk::Orientation::Vertical);
    separator.add_css_class("bar-separator");
    separator
}

/// Save to, timer, and what to show: radio groups and checks, stored in the options as they change.
fn options_menu(state: &Rc<RefCell<State>>) -> gtk::Popover {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.add_css_class("shot-menu");
    let options = state.borrow().options.clone();
    let heading = |text: &str| {
        let label = gtk::Label::new(Some(text));
        label.add_css_class("heading");
        label.set_xalign(0.0);
        label
    };
    column.append(&heading("SAVE TO"));
    let mut first: Option<gtk::CheckButton> = None;
    for (label, value) in [("Screenshots folder", SaveTo::Folder), ("Clipboard", SaveTo::Clipboard), ("Both", SaveTo::Both)] {
        let check = gtk::CheckButton::with_label(label);
        check.set_group(first.as_ref());
        check.set_active(options.save_to == value);
        let weak = Rc::downgrade(state);
        check.connect_toggled(move |check| {
            if check.is_active()
                && let Some(state) = weak.upgrade()
            {
                state.borrow_mut().options.save_to = value;
            }
        });
        first.get_or_insert(check.clone());
        column.append(&check);
    }
    column.append(&heading("TIMER"));
    let mut first: Option<gtk::CheckButton> = None;
    for (label, seconds) in [("None", 0), ("5 seconds", 5), ("10 seconds", 10)] {
        let check = gtk::CheckButton::with_label(label);
        check.set_group(first.as_ref());
        check.set_active(options.timer == seconds);
        let weak = Rc::downgrade(state);
        check.connect_toggled(move |check| {
            if check.is_active()
                && let Some(state) = weak.upgrade()
            {
                state.borrow_mut().options.timer = seconds;
            }
        });
        first.get_or_insert(check.clone());
        column.append(&check);
    }
    column.append(&heading("SHOW"));
    for (label, on, set) in [
        ("Pointer", options.pointer, (|options: &mut Options, on| options.pointer = on) as fn(&mut Options, bool)),
        ("Thumbnail afterwards", options.thumbnail, |options: &mut Options, on| options.thumbnail = on),
    ] {
        let check = gtk::CheckButton::with_label(label);
        check.set_active(on);
        let weak = Rc::downgrade(state);
        check.connect_toggled(move |check| {
            if let Some(state) = weak.upgrade() {
                set(&mut state.borrow_mut().options, check.is_active());
            }
        });
        column.append(&check);
    }
    let popover = gtk::Popover::new();
    popover.set_child(Some(&column));
    popover
}

#[derive(Clone, Copy)]
enum Glyph {
    Close,
    Mode(Mode),
    Record,
}

fn glyph(kind: Glyph, on: bool) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(22).content_height(18).build();
    set_glyph(&area, kind, on);
    area
}

/// The design's bar glyphs: a screen outline with a filled screen, window or dashed area inside.
fn set_glyph(area: &gtk::DrawingArea, kind: Glyph, on: bool) {
    area.set_draw_func(move |_, cr, _, _| {
        let palette = &autark_common::theme::look().palette;
        let colour = if on { palette.accent.clone() } else { palette.text.clone() };
        autark_common::theme::set_source(cr, &colour, 1.0);
        cr.set_line_width(1.5);
        match kind {
            Glyph::Close => {
                autark_common::theme::set_source(cr, &palette.text2, 1.0);
                cr.move_to(6.0, 4.0);
                cr.line_to(16.0, 14.0);
                cr.move_to(16.0, 4.0);
                cr.line_to(6.0, 14.0);
                let _ = cr.stroke();
            }
            Glyph::Mode(mode) => {
                if mode == Mode::Area {
                    cr.set_dash(&[2.5, 2.0], 0.0);
                }
                cr.rectangle(1.0, 1.0, 20.0, 16.0);
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
                let (x, y, w, h) = match mode {
                    Mode::Screen => (3.0, 3.0, 16.0, 12.0),
                    Mode::Window => (5.0, 4.0, 12.0, 10.0),
                    Mode::Area => (6.0, 5.0, 10.0, 8.0),
                };
                autark_common::theme::set_source(cr, &colour, if on { 0.35 } else { 0.18 });
                cr.rectangle(x, y, w, h);
                let _ = cr.fill();
            }
            Glyph::Record => {
                cr.rectangle(1.0, 1.0, 20.0, 16.0);
                let _ = cr.stroke();
                cr.arc(11.0, 9.0, 3.4, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        }
    });
    area.queue_draw();
}
