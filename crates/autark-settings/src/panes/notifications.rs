//! Notifications: Focus (do not disturb) through `autark-notifyd` live, and its `notifyd.toml` for how long banners stay,
//! how many show at once and Kith's suggestions in the widgets. The daemon reads that file when it starts, so those
//! three apply from the next login. The chime is on the Sound pane, and the lock screen's text on Fingerprint & Lock.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::{bus, config, ui};

#[zbus::proxy(interface = "org.autark.Notifications1", default_service = "org.autark.Notifications1", default_path = "/org/autark/Notifications")]
trait Notifications {
    fn set_focus(&self, on: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn focus(&self) -> zbus::Result<bool>;
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Notifications", None));
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

    let now = ui::group();
    let updating = Rc::new(Cell::new(true));
    let focus = ui::switch(false, {
        let (report, updating) = (report.clone(), updating.clone());
        move |on| {
            if updating.get() {
                return;
            }
            let report = report.clone();
            glib::spawn_future_local(async move {
                let result = async { NotificationsProxy::new(&bus::session().await?).await?.set_focus(on).await }.await;
                report(result.map_err(|error| bus::message(error, "The notification centre (autark-notifyd)")));
            });
        }
    });
    glib::spawn_future_local({
        let (focus, updating, report) = (focus.clone(), updating.clone(), report.clone());
        async move {
            let on = async { NotificationsProxy::new(&bus::session().await?).await?.focus().await }.await;
            match on {
                Ok(on) => focus.set_active(on),
                Err(error) => report(Err(bus::message(error, "The notification centre (autark-notifyd)"))),
            }
            updating.set(false);
        }
    });
    now.append(&ui::row("Focus", Some("Keep banners and sounds quiet; everything waits in the notification centre (Super+Shift+N)"), &focus));
    column.append(&ui::section("Now", None, &now));

    let document = config::read("notifyd");
    let banners = ui::group();
    let set = {
        let report = report.clone();
        Rc::new(move |key: &'static str, value: toml_edit::Value| report(config::edit("notifyd", |document| config::set_in(document, key, value))))
    };
    let seconds = config::get_int(&document, "banner_seconds").unwrap_or(5).clamp(1, 30);
    let shown = gtk::Label::new(Some(&format!("{seconds} s")));
    shown.add_css_class("value");
    shown.set_width_chars(6);
    shown.set_xalign(1.0);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let debounce = ui::Debounce::default();
    line.append(&ui::slider(1.0, 30.0, 1.0, seconds as f64, "", "", {
        let (shown, set) = (shown.clone(), set.clone());
        move |seconds| {
            let seconds = seconds.round() as i64;
            shown.set_label(&format!("{seconds} s"));
            let set = set.clone();
            debounce.run(Duration::from_millis(250), move || set("banner_seconds", seconds.into()));
        }
    }));
    line.append(&shown);
    banners.append(&ui::row("Banners stay for", Some("When the app doesn't say"), &line));

    let max = config::get_int(&document, "max_banners").unwrap_or(3).clamp(1, 8);
    let counts: Vec<(String, String)> = (1..=6).map(|count| (count.to_string(), count.to_string())).collect();
    let counts: Vec<(&str, &str)> = counts.iter().map(|(value, label)| (value.as_str(), label.as_str())).collect();
    banners.append(&ui::row(
        "Banners at once",
        Some("Older ones go straight to the centre"),
        &ui::choice(&counts, &max.to_string(), {
            let set = set.clone();
            move |count| set("max_banners", count.parse::<i64>().unwrap_or(3).into())
        }),
    ));
    banners.append(&ui::row(
        "Kith's suggestions in widgets",
        None,
        &ui::switch(config::get_bool(&document, "suggestions").unwrap_or(true), {
            let set = set.clone();
            move |on| set("suggestions", on.into())
        }),
    ));
    column.append(&ui::section("Banners", None, &banners));
    column.append(&ui::note("The notification daemon reads these when it starts, so a change shows after you log in again."));
    column.append(&status);
    page.upcast()
}
