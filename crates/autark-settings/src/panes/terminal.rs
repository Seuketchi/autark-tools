//! Terminal: edits `terminal.toml`, which open terminal windows watch, so a change shows at once. The desktop's own
//! light or dark and accent are on the Appearance pane; this is the terminal's font, cursor and colours.

use std::time::Duration;

use gtk::prelude::*;

use crate::{config, ui};

const SCHEMES: [(&str, &str); 3] = [("follow", "Match the desktop"), ("light", "Light"), ("dark", "Dark")];
const CURSORS: [(&str, &str); 3] = [("block", "Block"), ("bar", "Bar"), ("underline", "Underline")];
const FONTS: [(&str, &str); 2] = [("Geist Mono", "Geist Mono"), ("monospace", "System monospace")];
const SCROLLBACK: [(&str, &str); 4] = [("1000", "1,000 lines"), ("10000", "10,000 lines"), ("100000", "100,000 lines"), ("0", "None")];
const COLOURS: [(&str, &str, &str); 3] = [("foreground", "Text", "#3E3A34"), ("background", "Background", "#EFECE5"), ("cursor_color", "Cursor", "#24221E")];

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Terminal", Some("Dark mode and the accent colour follow Appearance; the rest is here.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);

    let document = config::read("terminal");
    let report = {
        let status = status.clone();
        move |result: Result<(), String>| {
            status.set_visible(result.is_err());
            if let Err(error) = result {
                status.set_label(&error);
            }
        }
    };
    let set = {
        let report = report.clone();
        move |key: &'static str, value: toml_edit::Value| report(config::edit("terminal", |document| config::set_in(document, key, value)))
    };

    let look = ui::group();
    let scheme = config::get_str(&document, "scheme").unwrap_or_else(|| "follow".into());
    look.append(&ui::row(
        "Colours",
        Some("Light or dark for the terminal's body, whatever the desktop is"),
        &ui::segmented(&SCHEMES, &scheme, {
            let set = set.clone();
            move |scheme| set("scheme", scheme.into())
        }),
    ));
    let font = config::get_str(&document, "font").unwrap_or_else(|| "Geist Mono".into());
    look.append(&ui::row(
        "Font",
        None,
        &ui::choice(&FONTS, &font, {
            let set = set.clone();
            move |font| set("font", font.into())
        }),
    ));
    look.append(&slider_row(
        "Text size",
        config::get_float(&document, "font_size").unwrap_or(13.0).clamp(8.0, 32.0),
        (8.0, 32.0, 1.0),
        |size| format!("{} px", size.round()),
        {
            let set = set.clone();
            move |size| set("font_size", size.round().into())
        },
    ));
    look.append(&slider_row(
        "Line spacing",
        config::get_float(&document, "line_height").unwrap_or(1.4).clamp(1.0, 2.0),
        (1.0, 2.0, 0.05),
        |height| format!("{height:.2}"),
        {
            let set = set.clone();
            move |height| set("line_height", ((height * 20.0).round() / 20.0).into())
        },
    ));
    column.append(&ui::section("Look", None, &look));

    let cursor = ui::group();
    let shape = config::get_str(&document, "cursor").unwrap_or_else(|| "block".into());
    cursor.append(&ui::row(
        "Shape",
        None,
        &ui::segmented(&CURSORS, &shape, {
            let set = set.clone();
            move |shape| set("cursor", shape.into())
        }),
    ));
    cursor.append(&ui::row(
        "Blink",
        None,
        &ui::switch(config::get_bool(&document, "cursor_blink").unwrap_or(false), {
            let set = set.clone();
            move |on| set("cursor_blink", on.into())
        }),
    ));
    let kept = config::get_int(&document, "scrollback").unwrap_or(10_000).to_string();
    cursor.append(&ui::row(
        "Scrollback",
        Some("Lines kept above the screen"),
        &ui::choice(&SCROLLBACK, &kept, {
            let set = set.clone();
            move |lines| set("scrollback", lines.parse::<i64>().unwrap_or(10_000).into())
        }),
    ));
    column.append(&ui::section("Cursor and history", None, &cursor));

    let colours = ui::group();
    let fields: Vec<(&'static str, gtk::Entry)> = COLOURS
        .iter()
        .map(|&(key, label, fallback)| {
            let entry = ui::field(&config::get_str(&document, key).unwrap_or_default(), 10, {
                let report = report.clone();
                move |text| {
                    let text = text.trim();
                    if text.is_empty() {
                        report(config::edit("terminal", |document| config::remove_in(document, key)));
                        return true;
                    }
                    if gtk::gdk::RGBA::parse(text).is_err() {
                        return false;
                    }
                    report(config::edit("terminal", |document| config::set_in(document, key, text)));
                    true
                }
            });
            entry.set_placeholder_text(Some(fallback));
            colours.append(&ui::row(label, None, &entry));
            (key, entry)
        })
        .collect();
    let reset = ui::button("Use the theme's colours", move |_| {
        for (key, entry) in &fields {
            entry.set_text("");
            entry.remove_css_class("invalid");
            report(config::edit("terminal", |document| config::remove_in(document, key)));
        }
    });
    colours.append(&ui::row("Reset", Some("Leave a field empty to use the theme's colour; the sixteen colours can be set with `palette` in terminal.toml"), &reset));
    column.append(&ui::section("Colours", None, &colours));
    column.append(&status);
    page.upcast()
}

/// A slider with its value beside it, writing 250 ms after it stops moving.
fn slider_row(label: &str, value: f64, (min, max, step): (f64, f64, f64), show: impl Fn(f64) -> String + 'static, write: impl Fn(f64) + Clone + 'static) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let shown = gtk::Label::new(Some(&show(value)));
    shown.add_css_class("value");
    shown.set_width_chars(6);
    shown.set_xalign(1.0);
    let debounce = ui::Debounce::default();
    line.append(&ui::slider(min, max, step, value, "", "", {
        let shown = shown.clone();
        move |value| {
            shown.set_label(&show(value));
            let write = write.clone();
            debounce.run(Duration::from_millis(250), move || write(value));
        }
    }));
    line.append(&shown);
    ui::row(label, None, &line)
}
