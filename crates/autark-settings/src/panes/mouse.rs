//! Mouse & Touchpad: Wayfire's `[input]` options for pointers, set live over its IPC and kept in `wayfire.ini` (see
//! `wayfire.rs`). Wayfire applies a change at once, so there's nothing to restart.

use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use crate::wayfire::{self, Ini};
use crate::ui;

const PROFILES: [(&str, &str); 3] = [("default", "Default"), ("adaptive", "Accelerate"), ("flat", "Flat")];
const CLICKS: [(&str, &str); 3] = [("default", "Default"), ("button-areas", "Button areas"), ("clickfinger", "Number of fingers")];
const SCROLLS: [(&str, &str); 3] = [("default", "Default"), ("two-finger", "Two fingers"), ("edge", "Along the edge")];

struct Mouse {
    status: gtk::Label,
    debounce: ui::Debounce,
}

fn read(key: &str) -> Option<String> {
    Ini::parse(&std::fs::read_to_string(wayfire::ini_path()).unwrap_or_default()).get("input", key).or_else(|| wayfire::get(&format!("input/{key}")))
}

fn flag(key: &str, fallback: bool) -> bool {
    read(key).map_or(fallback, |value| value.trim() == "true")
}

fn number(key: &str, fallback: f64) -> f64 {
    read(key).and_then(|value| value.trim().parse().ok()).unwrap_or(fallback)
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Mouse & Touchpad", Some("Wayfire applies these as you change them.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let pane = Rc::new(Mouse { status, debounce: ui::Debounce::default() });

    let mouse = ui::group();
    mouse.append(&ui::row("Pointer speed", None, &pane.speed("input/mouse_cursor_speed", number("mouse_cursor_speed", 0.0))));
    mouse.append(&ui::row("Acceleration", None, &pane.profile("input/mouse_accel_profile", &read("mouse_accel_profile").unwrap_or_else(|| "default".into()))));
    mouse.append(&ui::row("Scroll speed", None, &pane.scroll_speed("input/mouse_scroll_speed", number("mouse_scroll_speed", 1.0))));
    mouse.append(&ui::row("Natural scrolling", Some("Content moves with your finger or wheel"), &pane.toggle("input/mouse_natural_scroll", flag("mouse_natural_scroll", false))));
    mouse.append(&ui::row("Left-handed", Some("Swap the left and right buttons"), &pane.toggle("input/left_handed_mode", flag("left_handed_mode", false))));
    mouse.append(&ui::row("Middle click from both buttons", None, &pane.toggle("input/middle_emulation", flag("middle_emulation", false))));
    column.append(&ui::section("Mouse", None, &mouse));

    let pad = ui::group();
    pad.append(&ui::row("Pointer speed", None, &pane.speed("input/touchpad_cursor_speed", number("touchpad_cursor_speed", 0.0))));
    pad.append(&ui::row("Acceleration", None, &pane.profile("input/touchpad_accel_profile", &read("touchpad_accel_profile").unwrap_or_else(|| "default".into()))));
    pad.append(&ui::row("Scroll speed", None, &pane.scroll_speed("input/touchpad_scroll_speed", number("touchpad_scroll_speed", 1.0))));
    pad.append(&ui::row("Tap to click", None, &pane.toggle("input/tap_to_click", flag("tap_to_click", true))));
    pad.append(&ui::row("Tap and drag", Some("Tap, then drag with the same finger"), &pane.toggle("input/tap_and_drag", flag("tap_and_drag", true))));
    pad.append(&ui::row("Natural scrolling", None, &pane.toggle("input/natural_scroll", flag("natural_scroll", true))));
    pad.append(&ui::row("Scroll with", None, &pane.choose("input/scroll_method", &SCROLLS, "default")));
    pad.append(&ui::row("Click with", None, &pane.choose("input/click_method", &CLICKS, "default")));
    pad.append(&ui::row("Ignore while typing", Some("Stops a palm from moving the pointer"), &pane.toggle("input/disable_touchpad_while_typing", flag("disable_touchpad_while_typing", true))));
    pad.append(&ui::row("Ignore while a mouse is connected", None, &pane.toggle("input/disable_touchpad_while_mouse", flag("disable_touchpad_while_mouse", false))));
    column.append(&ui::section("Touchpad", None, &pad));

    let gestures = ui::group();
    gestures.append(&ui::row("Gesture sensitivity", Some("For swipes between workspaces"), &pane.gesture("input/gesture_sensitivity", number("gesture_sensitivity", 1.0))));
    column.append(&ui::section("Gestures", None, &gestures));
    column.append(&pane.status);
    page.upcast()
}

impl Mouse {
    fn set(&self, option: &'static str, value: String) {
        let result = wayfire::set(&[(option, value)]);
        self.status.set_visible(result.is_err());
        if let Err(error) = result {
            self.status.set_label(&error);
        }
    }

    fn toggle(self: &Rc<Self>, option: &'static str, on: bool) -> gtk::Switch {
        let this = self.clone();
        ui::switch(on, move |on| this.set(option, on.to_string()))
    }

    fn choose(self: &Rc<Self>, option: &'static str, options: &[(&str, &str)], fallback: &str) -> gtk::DropDown {
        let this = self.clone();
        let current = read(option.trim_start_matches("input/")).unwrap_or_else(|| fallback.to_owned());
        ui::choice(options, &current, move |value| this.set(option, value.to_owned()))
    }

    fn profile(self: &Rc<Self>, option: &'static str, current: &str) -> gtk::DropDown {
        let this = self.clone();
        ui::choice(&PROFILES, current, move |value| this.set(option, value.to_owned()))
    }

    /// A slider written 250 ms after it stops, with `step` of resolution.
    fn slider(self: &Rc<Self>, option: &'static str, (min, max, step): (f64, f64, f64), value: f64, left: &str, right: &str) -> gtk::Box {
        let this = self.clone();
        ui::slider(min, max, step, value.clamp(min, max), left, right, move |value| {
            let this = this.clone();
            let value = (value / step).round() * step;
            let again = this.clone();
            again.debounce.run(Duration::from_millis(250), move || this.set(option, format!("{value:.2}")));
        })
    }

    fn speed(self: &Rc<Self>, option: &'static str, value: f64) -> gtk::Box {
        self.slider(option, (-1.0, 1.0, 0.05), value, "Slow", "Fast")
    }

    fn scroll_speed(self: &Rc<Self>, option: &'static str, value: f64) -> gtk::Box {
        self.slider(option, (0.2, 3.0, 0.1), value, "Slow", "Fast")
    }

    fn gesture(self: &Rc<Self>, option: &'static str, value: f64) -> gtk::Box {
        self.slider(option, (0.1, 3.0, 0.1), value, "Low", "High")
    }
}
