//! Windows: how they open and close, where new ones go, and the title bars Wayfire draws. These are Wayfire's `animate`,
//! `place` and `decoration` options, set live over its IPC and kept in `wayfire.ini` (see `wayfire.rs`). Autark's own apps
//! draw their title bars themselves, so the title bar options reach other programs' windows. Reset removes the options
//! from the file, which puts Wayfire's own defaults back.

use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use crate::ui;
use crate::wayfire::{self, Ini};

const OPEN_CLOSE: [(&str, &str); 6] = [("none", "None"), ("fade", "Fade"), ("zoom", "Zoom"), ("fire", "Fire"), ("zap", "Zap"), ("spin", "Spin")];
const MINIMIZE: [(&str, &str); 3] = [("none", "None"), ("zoom", "Zoom"), ("squeezimize", "Squeeze")];
const PLACEMENT: [(&str, &str); 5] = [("center", "In the middle"), ("cascade", "Cascading"), ("pointer", "Under the pointer"), ("random", "Anywhere"), ("maximize", "Maximised")];

/// Every option this pane writes, with Wayfire's own default for it.
const OPTIONS: [(&str, &str); 7] = [
    ("animate/open_animation", "zoom"),
    ("animate/close_animation", "zoom"),
    ("animate/minimize_animation", "squeezimize"),
    ("animate/duration", "400ms"),
    ("place/mode", "center"),
    ("decoration/title_height", "30"),
    ("decoration/border_size", "4"),
];

struct Windows {
    status: gtk::Label,
    debounce: ui::Debounce,
    body: gtk::Box,
}

fn default_of(option: &str) -> &'static str {
    OPTIONS.iter().find(|(name, _)| *name == option).map_or("", |(_, default)| default)
}

/// An option as the file has it, else Wayfire's own default.
fn read(option: &str) -> String {
    let (section, key) = option.split_once('/').unwrap_or(("", option));
    Ini::parse(&std::fs::read_to_string(wayfire::ini_path()).unwrap_or_default()).get(section, key).unwrap_or_else(|| default_of(option).to_owned())
}

/// The milliseconds of an animation option, which reads `400ms` or `400ms circle` (the second word is the easing).
fn milliseconds(value: &str) -> f64 {
    value.split_whitespace().next().and_then(|time| time.trim_end_matches("ms").parse().ok()).unwrap_or(400.0)
}

/// `value` with its time replaced by `ms`, and its easing left as it was.
fn with_milliseconds(value: &str, ms: u32) -> String {
    let easing: Vec<&str> = value.split_whitespace().skip(1).collect();
    if easing.is_empty() { format!("{ms}ms") } else { format!("{ms}ms {}", easing.join(" ")) }
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Windows", Some("Wayfire applies these as you change them.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let pane = Rc::new(Windows { status, debounce: ui::Debounce::default(), body: body.clone() });
    pane.show();
    column.append(&body);
    column.append(&pane.status);
    let reset = ui::button("Reset to defaults", {
        let pane = pane.clone();
        move |_| pane.reset()
    });
    reset.set_halign(gtk::Align::Start);
    column.append(&reset);
    page.upcast()
}

impl Windows {
    fn report(&self, result: Result<(), String>) {
        self.status.set_visible(result.is_err());
        if let Err(error) = result {
            self.status.set_label(&error);
        }
    }

    fn set(&self, option: &'static str, value: String) {
        self.report(wayfire::set(&[(option, value)]));
    }

    fn show(self: &Rc<Self>) {
        ui::clear(&self.body);
        let animations = ui::group();
        animations.append(&ui::row("Opening", None, &self.choose("animate/open_animation", &OPEN_CLOSE)));
        animations.append(&ui::row("Closing", None, &self.choose("animate/close_animation", &OPEN_CLOSE)));
        animations.append(&ui::row("Minimising", None, &self.choose("animate/minimize_animation", &MINIMIZE)));
        animations.append(&ui::row("Speed", Some("How long they take"), &self.duration()));
        self.body.append(&ui::section("Animations", None, &animations));

        let placing = ui::group();
        placing.append(&ui::row("New windows open", None, &self.choose("place/mode", &PLACEMENT)));
        self.body.append(&ui::section("Placement", None, &placing));

        let bars = ui::group();
        bars.append(&ui::row("Title bar height", Some("For windows Wayfire draws the title bar of"), &self.size("decoration/title_height", (16.0, 56.0, 2.0))));
        bars.append(&ui::row("Border", None, &self.size("decoration/border_size", (0.0, 12.0, 1.0))));
        self.body.append(&ui::section("Title bars", None, &bars));
    }

    /// Takes the pane's options out of `wayfire.ini`, and Wayfire's own defaults are back.
    fn reset(self: &Rc<Self>) {
        let options: Vec<(&str, Option<&str>)> = OPTIONS.iter().map(|(option, _)| (*option, None)).collect();
        self.report(autark_common::ini::update(&wayfire::ini_path(), &options));
        // Live too, as Wayfire may not have noticed the file yet.
        let live: Vec<(&str, String)> = OPTIONS.iter().map(|(option, default)| (*option, (*default).to_owned())).collect();
        let _ = wayfire::set_live(&live);
        self.show();
    }

    fn choose(self: &Rc<Self>, option: &'static str, options: &[(&str, &str)]) -> gtk::DropDown {
        let this = self.clone();
        ui::choice(options, read(option).trim(), move |value| this.set(option, value.to_owned()))
    }

    fn duration(self: &Rc<Self>) -> gtk::Box {
        let this = self.clone();
        let current = read("animate/duration");
        ui::slider(100.0, 800.0, 50.0, milliseconds(&current).clamp(100.0, 800.0), "Quick", "Slow", move |value| {
            let this = this.clone();
            let ms = ((value / 50.0).round() * 50.0) as u32;
            let again = this.clone();
            // The easing is kept as the file has it.
            again.debounce.run(Duration::from_millis(250), move || this.set("animate/duration", with_milliseconds(&read("animate/duration"), ms)));
        })
    }

    fn size(self: &Rc<Self>, option: &'static str, (min, max, step): (f64, f64, f64)) -> gtk::Box {
        let this = self.clone();
        let current = read(option).trim().parse::<f64>().unwrap_or(0.0);
        ui::slider(min, max, step, current.clamp(min, max), "", "", move |value| {
            let this = this.clone();
            let value = ((value / step).round() * step) as i32;
            let again = this.clone();
            again.debounce.run(Duration::from_millis(250), move || this.set(option, value.to_string()));
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_times_keep_their_easing() {
        assert_eq!(milliseconds("400ms"), 400.0);
        assert_eq!(milliseconds("250ms circle"), 250.0);
        assert_eq!(milliseconds("nonsense"), 400.0);
        assert_eq!(with_milliseconds("400ms", 200), "200ms");
        assert_eq!(with_milliseconds("400ms circle", 200), "200ms circle");
    }

    #[test]
    fn every_option_has_a_default() {
        for (option, default) in OPTIONS {
            assert!(option.contains('/') && !default.is_empty(), "{option}");
        }
        assert_eq!(default_of("place/mode"), "center");
    }

    #[test]
    fn a_choice_includes_wayfires_own_default() {
        for (option, choices) in [("animate/open_animation", &OPEN_CLOSE[..]), ("animate/minimize_animation", &MINIMIZE[..]), ("place/mode", &PLACEMENT[..])] {
            assert!(choices.iter().any(|(value, _)| *value == default_of(option)), "{option}");
        }
    }
}
