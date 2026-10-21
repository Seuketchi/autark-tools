//! The bar's items and clock, the dock's apps and the launcher's results: the "Menu bar", "Dock apps" and "Launcher" sections of
//! Desktop & Dock. They write `bar.toml`'s `start`, `end` and `clock_format`, `dock.toml`'s `pinned` and `launcher.toml`, which the
//! bar and the dock watch (the launcher reads its file each time it opens), so a change shows at once. "Reset" removes the keys from the file, and the shipped bar and dock are back.

use std::rc::Rc;

use autark_common::layout;
use gtk::prelude::*;
use toml_edit::{Array, Value};

use crate::{config, ui};

type Report = Rc<dyn Fn(Result<(), String>)>;

/// `list` with the item at `index` moved by `delta` places, if there is room to.
fn moved(list: &[String], index: usize, delta: isize) -> Vec<String> {
    let mut list = list.to_vec();
    let target = index.checked_add_signed(delta).filter(|target| *target < list.len());
    if let (Some(target), true) = (target, index < list.len()) {
        list.swap(index, target);
    }
    list
}

/// The names of a side as the file lists them, else the default, with every item that is left out added after as hidden:
/// `(name, shown)` for the whole of `known`.
fn sides(wanted: Option<Vec<String>>, known: &[(&'static str, &str)]) -> Vec<(&'static str, bool)> {
    let default = layout::names(known);
    let shown = layout::resolve(&wanted.unwrap_or(default), known).0;
    let mut all: Vec<(&'static str, bool)> = shown.iter().map(|name| (*name, true)).collect();
    all.extend(known.iter().filter(|(name, _)| !shown.contains(name)).map(|(name, _)| (*name, false)));
    all
}

fn array(document: &toml_edit::DocumentMut, key: &str) -> Option<Vec<String>> {
    document.get(key)?.as_array().map(|values| values.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
}

fn write_array(report: &Report, name: &'static str, key: &'static str, values: &[String]) {
    let mut list = Array::new();
    for value in values {
        list.push(value.as_str());
    }
    report(config::edit(name, |document| config::set_in(document, key, Value::Array(list))));
}

fn arrow(label: &str, name: &str, enabled: bool, clicked: impl Fn() + 'static) -> gtk::Button {
    let button = ui::button(label, move |_| clicked());
    button.set_sensitive(enabled);
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button
}

/// The bar's two lists of items and its clock.
pub fn bar(report: &Report) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let fill = Rc::new(std::cell::RefCell::new(None::<Box<dyn Fn()>>));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&body);
    let rebuild: Rc<dyn Fn()> = {
        let (body, report) = (body.clone(), report.clone());
        let fill = fill.clone();
        Rc::new(move || {
            ui::clear(&body);
            let document = config::read("bar");
            body.append(&ui::section("Menu bar: left", None, &side(&report, &fill, "start", array(&document, "start"), &layout::START)));
            body.append(&ui::section("Menu bar: right", None, &side(&report, &fill, "end", array(&document, "end"), &layout::END)));
            body.append(&ui::section("Clock", None, &clock(&report, &document)));
        })
    };
    let again = rebuild.clone();
    *fill.borrow_mut() = Some(Box::new(move || again()));
    rebuild();
    let reset = ui::button("Reset the menu bar", {
        let (report, rebuild) = (report.clone(), rebuild.clone());
        move |_| {
            report(config::edit("bar", |document| {
                for key in ["start", "end", "clock_format"] {
                    config::remove_in(document, key);
                }
            }));
            rebuild();
        }
    });
    reset.set_halign(gtk::Align::Start);
    column.append(&reset);
    column
}

type Refresh = Rc<std::cell::RefCell<Option<Box<dyn Fn()>>>>;

fn refresh(fill: &Refresh) {
    if let Some(again) = fill.borrow().as_ref() {
        again();
    }
}

/// One side: every item that can be there, shown or not, with a switch and arrows.
fn side(report: &Report, fill: &Refresh, key: &'static str, wanted: Option<Vec<String>>, known: &'static [(&'static str, &'static str)]) -> gtk::Box {
    let group = ui::group();
    let all = sides(wanted, known);
    let shown: Vec<String> = all.iter().filter(|(_, on)| *on).map(|(name, _)| (*name).to_owned()).collect();
    for &(name, on) in &all {
        let label = known.iter().find(|(candidate, _)| *candidate == name).map_or(name, |(_, label)| *label);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let position = shown.iter().position(|shown| shown == name);
        let (up, down) = (
            arrow("↑", &format!("Move {label} earlier"), position.is_some_and(|index| index > 0), {
                let (report, fill, shown) = (report.clone(), fill.clone(), shown.clone());
                move || {
                    write_array(&report, "bar", key, &moved(&shown, position.unwrap_or(0), -1));
                    refresh(&fill);
                }
            }),
            arrow("↓", &format!("Move {label} later"), position.is_some_and(|index| index + 1 < shown.len()), {
                let (report, fill, shown) = (report.clone(), fill.clone(), shown.clone());
                move || {
                    write_array(&report, "bar", key, &moved(&shown, position.unwrap_or(0), 1));
                    refresh(&fill);
                }
            }),
        );
        line.append(&up);
        line.append(&down);
        let switch = ui::switch(on, {
            let (report, fill, shown) = (report.clone(), fill.clone(), shown.clone());
            move |on| {
                let mut next = shown.clone();
                if on {
                    next.push(name.to_owned());
                } else {
                    next.retain(|item| item != name);
                }
                write_array(&report, "bar", key, &next);
                refresh(&fill);
            }
        });
        line.append(&switch);
        group.append(&ui::row(label, None, &line));
    }
    group
}

fn clock(report: &Report, document: &toml_edit::DocumentMut) -> gtk::Box {
    let group = ui::group();
    let format = config::get_str(document, "clock_format").unwrap_or_else(|| layout::CLOCKS[0].2.to_owned());
    let preset = layout::clock_preset(&format);
    let mut choices: Vec<(&str, &str)> = layout::CLOCKS.iter().map(|(id, label, _)| (*id, *label)).collect();
    choices.push(("custom", "Custom"));
    let entry = gtk::Entry::builder().text(&format).hexpand(true).build();
    entry.add_css_class("field");
    entry.set_visible(preset == "custom");
    let picker = ui::choice(&choices, preset, {
        let (report, entry) = (report.clone(), entry.clone());
        move |id| match layout::CLOCKS.iter().find(|(candidate, ..)| *candidate == id) {
            Some((.., format)) => {
                entry.set_visible(false);
                report(config::edit("bar", |document| config::set_in(document, "clock_format", *format)));
            }
            None => entry.set_visible(true),
        }
    });
    group.append(&ui::row("Show", None, &picker));
    entry.connect_changed({
        let report = report.clone();
        move |entry| {
            let text = entry.text().to_string();
            let valid = !text.is_empty() && chrono::format::StrftimeItems::new(&text).all(|item| !matches!(item, chrono::format::Item::Error));
            if valid {
                entry.remove_css_class("invalid");
                report(config::edit("bar", |document| config::set_in(document, "clock_format", text.as_str())));
            } else {
                entry.add_css_class("invalid");
            }
        }
    });
    group.append(&ui::row("Custom format", Some("strftime: %H:%M is 14:32, %a is Fri"), &entry));
    group
}

/// The dock's pinned apps, in order, with the means to add an installed app.
pub fn dock_apps(report: &Report) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&body);
    let fill: Refresh = Rc::default();
    let rebuild: Rc<dyn Fn()> = {
        let (body, report, fill) = (body.clone(), report.clone(), fill.clone());
        Rc::new(move || {
            ui::clear(&body);
            let pinned = array(&config::read("dock"), "pinned").unwrap_or_else(|| layout::names_of(&layout::PINNED));
            body.append(&ui::section("Dock apps", None, &apps(&report, &fill, &pinned)));
        })
    };
    let again = rebuild.clone();
    *fill.borrow_mut() = Some(Box::new(move || again()));
    rebuild();
    let reset = ui::button("Reset the dock's apps", {
        let (report, rebuild) = (report.clone(), rebuild.clone());
        move |_| {
            report(config::edit("dock", |document| config::remove_in(document, "pinned")));
            rebuild();
        }
    });
    reset.set_halign(gtk::Align::Start);
    column.append(&reset);
    column
}

fn apps(report: &Report, fill: &Refresh, pinned: &[String]) -> gtk::Box {
    let group = ui::group();
    let installed = autark_common::desktop::installed();
    let name_of = |id: &str| installed.iter().find(|(candidate, _)| candidate == id).map_or_else(|| format!("{id} (not installed)"), |(_, name)| name.clone());
    for (index, id) in pinned.iter().enumerate() {
        let label = name_of(id);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let change = |delta: isize| {
            let (report, fill, pinned) = (report.clone(), fill.clone(), pinned.to_vec());
            move || {
                write_array(&report, "dock", "pinned", &moved(&pinned, index, delta));
                refresh(&fill);
            }
        };
        line.append(&arrow("↑", &format!("Move {label} earlier"), index > 0, change(-1)));
        line.append(&arrow("↓", &format!("Move {label} later"), index + 1 < pinned.len(), change(1)));
        line.append(&arrow("Remove", &format!("Unpin {label}"), true, {
            let (report, fill, pinned) = (report.clone(), fill.clone(), pinned.to_vec());
            move || {
                let mut next = pinned.clone();
                next.remove(index);
                write_array(&report, "dock", "pinned", &next);
                refresh(&fill);
            }
        }));
        group.append(&ui::row(&label, None, &line));
    }
    // Apps not pinned yet, to add the one picked.
    let rest: Vec<(String, String)> = installed.iter().filter(|(id, _)| !pinned.contains(id)).cloned().collect();
    let mut choices: Vec<(&str, &str)> = vec![("", "Add an app…")];
    choices.extend(rest.iter().map(|(id, name)| (id.as_str(), name.as_str())));
    let add = ui::choice(&choices, "", {
        let (report, fill, pinned) = (report.clone(), fill.clone(), pinned.to_vec());
        move |id| {
            if id.is_empty() {
                return;
            }
            let mut next = pinned.clone();
            next.push(id.to_owned());
            write_array(&report, "dock", "pinned", &next);
            refresh(&fill);
        }
    });
    group.append(&ui::row("Pin another", None, &add));
    group
}

/// What the launcher lists: how many apps and files, and whether it finds the system actions.
pub fn launcher(report: &Report) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&body);
    let rebuild: Rc<dyn Fn()> = {
        let (body, report) = (body.clone(), report.clone());
        Rc::new(move || {
            ui::clear(&body);
            let document = config::read("launcher");
            let group = ui::group();
            let debounce = ui::Debounce::default();
            for (key, label, note, (min, max), fallback) in [("max_apps", "Apps listed", "At most this many in a search", (1, 12), 6), ("max_files", "Recent files listed", "0 leaves them out", (0, 10), 4)] {
                let value = config::get_int(&document, key).unwrap_or(fallback).clamp(min, max);
                let (report, debounce) = (report.clone(), debounce.clone());
                let slider = ui::slider(min as f64, max as f64, 1.0, value as f64, &min.to_string(), &max.to_string(), move |value| {
                    let (report, number) = (report.clone(), value.round() as i64);
                    debounce.run(std::time::Duration::from_millis(250), move || report(config::edit("launcher", |document| config::set_in(document, key, number))));
                });
                group.append(&ui::row(label, Some(note), &slider));
            }
            let actions = ui::switch(config::get_bool(&document, "actions").unwrap_or(true), {
                let report = report.clone();
                move |on| report(config::edit("launcher", |document| config::set_in(document, "actions", on)))
            });
            group.append(&ui::row("Find system actions", Some("Lock, sleep, restart, shut down, log out"), &actions));
            body.append(&ui::section("Launcher", None, &group));
        })
    };
    rebuild();
    let reset = ui::button("Reset the launcher", {
        let (report, rebuild) = (report.clone(), rebuild.clone());
        move |_| {
            report(config::edit("launcher", |document| {
                for key in ["max_apps", "max_files", "actions"] {
                    config::remove_in(document, key);
                }
            }));
            rebuild();
        }
    });
    reset.set_halign(gtk::Align::Start);
    column.append(&reset);
    column
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn items_move_one_place_and_stop_at_the_ends() {
        let items = list(&["a", "b", "c"]);
        assert_eq!(moved(&items, 1, -1), ["b", "a", "c"]);
        assert_eq!(moved(&items, 1, 1), ["a", "c", "b"]);
        assert_eq!(moved(&items, 0, -1), items);
        assert_eq!(moved(&items, 2, 1), items);
        assert_eq!(moved(&items, 9, 1), items);
    }

    #[test]
    fn a_side_lists_the_shown_items_first_then_the_hidden() {
        let all = sides(Some(list(&["clock", "wifi"])), &layout::END);
        assert_eq!(&all[..2], &[("clock", true), ("wifi", true)]);
        assert!(all[2..].iter().all(|(_, on)| !on) && all.len() == layout::END.len());
        assert!(sides(None, &layout::END).iter().all(|(_, on)| *on), "no setting shows everything");
    }
}
