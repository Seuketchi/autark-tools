//! Accessibility: bigger text and pointer, more contrast, less motion, and the screen magnifier. Text size, contrast and
//! motion are `theme.toml`, the same keys as Appearance (every app restyles live); the pointer is Wayfire's `cursor_size`.

use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use crate::wayfire::{self, Ini};
use crate::{config, ui};

const TEXT_SIZES: [(&str, &str); 3] = [("100", "Normal"), ("115", "Large"), ("130", "Larger")];

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Accessibility", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let report = {
        let status = status.clone();
        Rc::new(move |result: Result<(), String>| {
            status.set_visible(result.is_err());
            if let Err(error) = result {
                status.set_label(&error);
            }
        })
    };

    let theme = config::read("theme");
    let seeing = ui::group();
    let size = config::get_int(&theme, "text_size").unwrap_or(100).to_string();
    seeing.append(&ui::row(
        "Text size",
        Some("For every Autark app"),
        &ui::segmented(&TEXT_SIZES, &size, {
            let report = report.clone();
            move |size| report(config::edit("theme", |document| config::set_in(document, "text_size", size.parse::<i64>().unwrap_or(100))))
        }),
    ));
    for (key, label, note) in [("contrast", "Increase contrast", "Darker text and firmer borders"), ("reduce_motion", "Reduce motion", "No sliding or fading")] {
        let report = report.clone();
        let switch = ui::switch(config::get_bool(&theme, key).unwrap_or(false), move |on| report(config::edit("theme", |document| config::set_in(document, key, on))));
        seeing.append(&ui::row(label, Some(note), &switch));
    }

    let cursor: f64 = Ini::parse(&std::fs::read_to_string(wayfire::ini_path()).unwrap_or_default())
        .get("input", "cursor_size")
        .or_else(|| wayfire::get("input/cursor_size"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(24.0);
    let debounce = ui::Debounce::default();
    let shown = ui::value(&format!("{} px", cursor.round()));
    shown.set_width_chars(6);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.append(&ui::slider(16.0, 64.0, 8.0, cursor.clamp(16.0, 64.0), "", "", {
        let (report, shown) = (report.clone(), shown.clone());
        move |size| {
            let size = size.round() as i64;
            shown.set_label(&format!("{size} px"));
            let report = report.clone();
            debounce.run(Duration::from_millis(250), move || report(wayfire::set(&[("input/cursor_size", size.to_string())])));
        }
    }));
    line.append(&shown);
    seeing.append(&ui::row("Pointer size", Some("Apps pick it up when they next open"), &line));
    column.append(&ui::section("Seeing", None, &seeing));

    let zoom = ui::group();
    zoom.append(&ui::row("Zoom the screen", Some("Hold Super and scroll the mouse wheel or two fingers"), &ui::value("Super + scroll")));
    zoom.append(&ui::row("Move windows without the mouse", Some("Super + left button drag to move, right button drag to resize"), &ui::value("")));
    column.append(&ui::section("Magnifier and windows", None, &zoom));
    column.append(&status);
    page.upcast()
}
