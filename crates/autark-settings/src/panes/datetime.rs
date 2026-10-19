//! Date & Time: the time now, the time zone, automatic time from the network, and whether the bar's clock is 24-hour.
//! The zone and automatic time are `timedatectl`'s, which asks polkit; when that says no, the pane shows what it said
//! and nothing changes. The bar's clock format is `bar.toml`, which the bar watches.

use std::process::Command;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::{config, ui};

const TWENTY_FOUR: &str = "%a %-d %b  %H:%M";
const TWELVE: &str = "%a %-d %b  %-I:%M %p";

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Date & Time", None));
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

    let now = gtk::Label::new(None);
    now.add_css_class("value");
    let tick = {
        let now = now.downgrade();
        move || {
            let Some(now) = now.upgrade() else { return glib::ControlFlow::Break };
            if let Ok(time) = glib::DateTime::now_local() {
                now.set_label(&time.format("%A %-d %B %Y, %H:%M:%S").map(|text| text.to_string()).unwrap_or_default());
            }
            glib::ControlFlow::Continue
        }
    };
    tick();
    glib::timeout_add_seconds_local(1, tick);

    let state = timedate();
    let group = ui::group();
    group.append(&ui::row("Now", None, &now));

    let ntp = ui::switch(state.get("NTP").is_some_and(|value| value == "yes"), {
        let report = report.clone();
        move |on| report(run(&["timedatectl", "set-ntp", if on { "true" } else { "false" }]))
    });
    let synced = if state.get("NTPSynchronized").is_some_and(|value| value == "yes") { "Synchronised with a time server" } else { "Not synchronised yet" };
    group.append(&ui::row("Set time automatically", Some(synced), &ntp));

    let zones = zones();
    let list = gtk::StringList::new(&zones.iter().map(String::as_str).collect::<Vec<_>>());
    let zone = gtk::DropDown::new(Some(list), gtk::Expression::NONE);
    zone.set_enable_search(true);
    zone.set_expression(Some(&gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    zone.add_css_class("choice");
    let current = state.get("Timezone").cloned().unwrap_or_default();
    if let Some(index) = zones.iter().position(|zone| *zone == current) {
        zone.set_selected(index as u32);
    }
    zone.connect_selected_notify({
        let (report, zones, current) = (report.clone(), zones.clone(), std::cell::RefCell::new(current));
        move |zone| {
            let Some(chosen) = zones.get(zone.selected() as usize) else { return };
            if *current.borrow() == *chosen {
                return;
            }
            let result = run(&["timedatectl", "set-timezone", chosen]);
            if result.is_ok() {
                *current.borrow_mut() = chosen.clone();
            }
            report(result);
        }
    });
    group.append(&ui::row("Time zone", Some("Type to search"), &zone));
    column.append(&ui::section("Clock", None, &group));

    let bar = ui::group();
    let format = config::get_str(&config::read("bar"), "clock_format").unwrap_or_else(|| TWENTY_FOUR.to_owned());
    let twelve = ui::switch(format.contains("%p") || format.contains("%I") || format.contains("%-I"), {
        let report = report.clone();
        move |on| report(config::edit("bar", |document| config::set_in(document, "clock_format", if on { TWELVE } else { TWENTY_FOUR })))
    });
    bar.append(&ui::row("12-hour clock in the top bar", Some("Otherwise 24-hour"), &twelve));
    column.append(&ui::section("Top bar", None, &bar));
    column.append(&status);
    page.upcast()
}

/// `timedatectl show` as key and value.
fn timedate() -> std::collections::HashMap<String, String> {
    let output = Command::new("timedatectl").arg("show").output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    output.lines().filter_map(|line| line.split_once('=')).map(|(key, value)| (key.to_owned(), value.to_owned())).collect()
}

fn zones() -> Vec<String> {
    let output = Command::new("timedatectl").arg("list-timezones").output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    output.lines().map(str::to_owned).collect()
}

/// Runs a command; its complaint is the error.
fn run(command: &[&str]) -> Result<(), String> {
    let output = Command::new(command[0]).args(&command[1..]).output().map_err(|error| format!("{}: {error}", command[0]))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if said.is_empty() { format!("{} failed", command.join(" ")) } else { format!("{said} (changing this needs permission this session doesn't have)") })
}
