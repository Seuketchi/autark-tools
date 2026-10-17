//! Clock and calendar: the time, the date and this month with today marked.

use std::cell::Cell;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::shared::Shared;

/// The local time, updated on every minute.
pub fn source() -> Shared<glib::DateTime> {
    let now = Shared::new(local_now());
    tick(now.clone());
    now
}

pub fn local_now() -> glib::DateTime {
    glib::DateTime::now_local().expect("no local time")
}

fn tick(now: Shared<glib::DateTime>) {
    let time = local_now();
    // Timed afresh each minute, so a late wake after sleep doesn't leave the clock behind.
    let into_minute = u64::try_from(time.second()).unwrap_or(0) * 1000 + u64::try_from(time.microsecond()).unwrap_or(0) / 1000;
    now.set(time);
    glib::timeout_add_local_once(Duration::from_millis(60_050 - into_minute.min(60_000)), move || tick(now));
}

/// The card: compact for the centre, larger for the desktop.
pub fn view(now: &Shared<glib::DateTime>, large: bool) -> gtk::Box {
    let weekday = gtk::Label::builder().xalign(0.0).build();
    weekday.add_css_class("weekday");
    let time = gtk::Label::builder().xalign(0.0).build();
    time.add_css_class("time");
    let date = gtk::Label::builder().xalign(0.0).build();
    date.add_css_class("date");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(!large);
    text.set_valign(if large { gtk::Align::Fill } else { gtk::Align::Center });
    text.append(&weekday);
    text.append(&time);
    text.append(&date);
    if large {
        time.set_vexpand(true);
        time.set_valign(gtk::Align::Center);
    }

    let calendar = gtk::Grid::builder().column_homogeneous(true).row_spacing(2).column_spacing(2).valign(gtk::Align::Center).build();
    calendar.add_css_class("calendar");
    calendar.set_hexpand(large);

    let card = gtk::Box::new(gtk::Orientation::Horizontal, if large { 18 } else { 14 });
    card.add_css_class("card");
    card.add_css_class("clock");
    if large {
        card.add_css_class("large");
    }
    card.append(&text);
    card.append(&calendar);

    let shown_day = Cell::new(0);
    now.watch(&card, move |_, now| {
        weekday.set_label(&now.format("%A").map(|day| day.to_uppercase()).unwrap_or_default());
        time.set_label(&now.format("%H:%M").unwrap_or_default());
        let format = if large { "%-d %B %Y" } else { "%-d %B %Y  ·  week %V" };
        date.set_label(&now.format(format).unwrap_or_default());
        let day = now.year() * 10_000 + now.month() * 100 + now.day_of_month();
        if shown_day.replace(day) != day {
            fill_month(&calendar, now);
        }
    });
    card
}

fn fill_month(calendar: &gtk::Grid, now: &glib::DateTime) {
    while let Some(child) = calendar.first_child() {
        calendar.remove(&child);
    }
    for (column, name) in ["M", "T", "W", "T", "F", "S", "S"].into_iter().enumerate() {
        let label = gtk::Label::new(Some(name));
        label.add_css_class("day-name");
        calendar.attach(&label, column as i32, 0, 1, 1);
    }
    let Ok(first) = glib::DateTime::from_local(now.year(), now.month(), 1, 0, 0, 0.0) else {
        return;
    };
    let days = first.add_months(1).and_then(|next| next.add_days(-1)).map_or(30, |last| last.day_of_month());
    // Weeks start on Monday, day 1 of `day_of_week`.
    let offset = first.day_of_week() - 1;
    for day in 1..=days {
        let cell = offset + day - 1;
        let label = gtk::Label::new(Some(&day.to_string()));
        label.add_css_class("day");
        if day == now.day_of_month() {
            label.add_css_class("today");
        }
        calendar.attach(&label, cell % 7, 1 + cell / 7, 1, 1);
    }
}
