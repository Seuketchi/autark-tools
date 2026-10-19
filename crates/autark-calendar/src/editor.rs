//! The event editor: title, calendar, day, times, place and notes, with Save and, for an existing event, Delete.

use std::rc::Rc;

use chrono::{Duration, NaiveDate};
use gtk::prelude::*;
use gtk::{glib, pango};

use crate::backend::Cal;
use crate::logic::{self, Showing};
use crate::window::{Calendar, default_times};

fn field(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let name = gtk::Label::builder().label(label).xalign(0.0).build();
    name.add_css_class("field-label");
    row.append(&name);
    row.append(widget);
    row
}

/// Opens the editor for a new event on `day`, or for `existing`.
pub fn open(calendar: &Rc<Calendar>, existing: Option<Showing>, day: NaiveDate) {
    let calendars: Vec<Cal> = calendar.backend.snapshot();
    if calendars.is_empty() {
        return;
    }
    let found = existing.as_ref().and_then(|showing| calendar.backend.find(&showing.resource, &showing.uid));
    let item = found.as_ref().map(|(_, item)| item.clone());

    let window = gtk::Window::builder().title(if existing.is_some() { "Edit event" } else { "New event" }).transient_for(&calendar.window).modal(true).default_width(420).resizable(false).build();
    window.add_css_class("editor");

    let title = gtk::Entry::builder().placeholder_text("Title").text(item.as_ref().map(|item| item.summary.as_str()).unwrap_or_default()).build();
    let names: Vec<String> = calendars.iter().map(|cal| format!("{} — {}", cal.account_name, cal.name)).collect();
    let names_ref: Vec<&str> = names.iter().map(String::as_str).collect();
    let which = gtk::DropDown::from_strings(&names_ref);
    if let Some((cal, _)) = &found {
        which.set_selected(calendars.iter().position(|other| other.url == cal.url && other.account == cal.account).unwrap_or(0) as u32);
        which.set_sensitive(false);
    }
    let all_day = gtk::CheckButton::with_label("All day");
    all_day.set_active(existing.as_ref().is_some_and(|showing| showing.all_day));
    let date = gtk::Entry::builder().text(day.format("%Y-%m-%d").to_string()).build();
    let (default_start, default_end) = default_times();
    let (start_text, end_text) = match &existing {
        Some(showing) if !showing.all_day => (showing.start.format("%H:%M").to_string(), showing.end.format("%H:%M").to_string()),
        _ => (default_start.format("%H:%M").to_string(), default_end.format("%H:%M").to_string()),
    };
    let start = gtk::Entry::builder().text(start_text).max_width_chars(6).build();
    let end = gtk::Entry::builder().text(end_text).max_width_chars(6).build();
    let times = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    times.append(&field("Starts", &start));
    times.append(&field("Ends", &end));
    times.set_sensitive(!all_day.is_active());
    let times_ref = times.clone();
    all_day.connect_toggled(move |check| times_ref.set_sensitive(!check.is_active()));
    let place = gtk::Entry::builder().placeholder_text("Place").text(item.as_ref().map(|item| item.location.as_str()).unwrap_or_default()).build();
    let notes = gtk::TextView::builder().wrap_mode(gtk::WrapMode::Word).height_request(80).build();
    notes.buffer().set_text(item.as_ref().map(|item| item.description.as_str()).unwrap_or_default());
    let notes_frame = gtk::ScrolledWindow::builder().child(&notes).min_content_height(80).build();
    let error = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
    error.add_css_class("status");
    error.add_css_class("error");

    let save = gtk::Button::with_label("Save");
    save.add_css_class("primary");
    let cancel = gtk::Button::with_label("Cancel");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    if existing.is_some() {
        let delete = gtk::Button::with_label("Delete");
        delete.add_css_class("danger");
        buttons.append(&delete);
        let (me, window, error, showing) = (Rc::downgrade(calendar), window.clone(), error.clone(), existing.clone());
        delete.connect_clicked(move |button| {
            let (Some(me), Some(showing)) = (me.upgrade(), showing.clone()) else { return };
            button.set_sensitive(false);
            let (backend, resource) = (me.backend.clone(), showing.resource.clone());
            let (window, error, button, day) = (window.clone(), error.clone(), button.clone(), showing.start.date());
            glib::spawn_future_local(async move {
                match blocking::unblock(move || backend.delete(&resource)).await {
                    Ok(()) => {
                        window.close();
                        me.changed(day);
                    }
                    Err(why) => {
                        error.set_text(&why);
                        button.set_sensitive(true);
                    }
                }
            });
        });
    }
    buttons.append(&cancel);
    buttons.append(&save);
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());

    let form = gtk::Box::new(gtk::Orientation::Vertical, 12);
    form.set_margin_top(18);
    form.set_margin_bottom(18);
    form.set_margin_start(18);
    form.set_margin_end(18);
    form.append(&title);
    form.append(&field("Calendar", &which));
    form.append(&field("Day", &date));
    form.append(&all_day);
    form.append(&times);
    form.append(&place);
    form.append(&field("Notes", &notes_frame));
    form.append(&error);
    form.append(&buttons);
    window.set_child(Some(&form));
    window.set_default_widget(Some(&save));

    let (me, window_for_save, existing, calendars) = (Rc::downgrade(calendar), window.clone(), existing, calendars);
    save.connect_clicked(move |button| {
        let Some(me) = me.upgrade() else { return };
        let summary = title.text().trim().to_owned();
        if summary.is_empty() {
            error.set_text("Give the event a title.");
            return;
        }
        let Ok(day) = NaiveDate::parse_from_str(date.text().trim(), "%Y-%m-%d") else {
            error.set_text("The day should look like 2026-10-19.");
            return;
        };
        let times = if all_day.is_active() {
            None
        } else {
            match (logic::parse_time(&start.text()), logic::parse_time(&end.text())) {
                (Some(from), Some(to)) => Some((from, to)),
                _ => {
                    error.set_text("Times should look like 09:30.");
                    return;
                }
            }
        };
        let text = notes.buffer().text(&notes.buffer().start_iter(), &notes.buffer().end_iter(), false).to_string();
        let uid = existing.as_ref().map(|showing| showing.uid.clone()).unwrap_or_else(new_uid);
        let mut event = logic::new_event(&uid, &summary, day, times, 1);
        // An all-day event keeps its length when edited.
        if let (Some(old), true) = (existing.as_ref(), all_day.is_active()) {
            event.end = event.start.as_ref().map(|start| start.moved(start.naive() + (old.end - old.start).max(Duration::days(1))));
        }
        event.location = place.text().trim().to_owned();
        event.description = text.trim().to_owned();
        let chosen = calendars[which.selected() as usize].clone();
        let backend = me.backend.clone();
        let resource = existing.as_ref().map(|showing| showing.resource.clone());
        button.set_sensitive(false);
        let (window, error, button) = (window_for_save.clone(), error.clone(), button.clone());
        glib::spawn_future_local(async move {
            let result = blocking::unblock(move || match resource {
                Some(resource) => backend.update(&resource, event),
                None => backend.create(&chosen.account, &chosen.url, event),
            })
            .await;
            match result {
                Ok(()) => {
                    window.close();
                    me.changed(day);
                }
                Err(why) => {
                    error.set_text(&why);
                    button.set_sensitive(true);
                }
            }
        });
    });
    window.present();
}

/// An ID no other event has: the time to the nanosecond and 64 random bits.
pub fn new_uid() -> String {
    let mut random = [0u8; 8];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut file, &mut random);
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
    format!("{nanos:x}-{:016x}@autark", u64::from_le_bytes(random))
}
