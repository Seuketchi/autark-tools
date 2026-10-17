//! Desktop & Dock: edits `dock.toml`, which the dock watches, and the widgets' "show on desktop" in the
//! notification centre's own state file, which it watches too.

use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::{bus, config, ui};

#[zbus::proxy(interface = "org.autark.Notifications1", default_service = "org.autark.Notifications1", default_path = "/org/autark/Notifications")]
trait Notifications {
    fn edit_widgets(&self) -> zbus::Result<()>;
}

const POSITIONS: [(&str, &str); 3] = [("left", "Left"), ("bottom", "Bottom"), ("right", "Right")];

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Desktop & Dock", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);

    let document = config::read("dock");
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
        move |key: &'static str, value: toml_edit::Value| report(config::edit("dock", |document| config::set_in(document, key, value)))
    };

    let dock = ui::group();
    let size = config::get_int(&document, "size").unwrap_or(52).clamp(32, 80);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let value = gtk::Label::new(Some(&format!("{size} px")));
    value.add_css_class("value");
    value.set_width_chars(6);
    value.set_xalign(1.0);
    let debounce = ui::Debounce::default();
    line.append(&ui::slider(32.0, 80.0, 1.0, size as f64, "", "", {
        let (value, set) = (value.clone(), set.clone());
        move |size| {
            let size = size.round() as i64;
            value.set_label(&format!("{size} px"));
            let set = set.clone();
            debounce.run(Duration::from_millis(250), move || set("size", size.into()));
        }
    }));
    line.append(&value);
    dock.append(&ui::row("Size", None, &line));

    for (key, label, fallback) in [("magnify", "Magnify icons on hover", true), ("dots", "Show dots under running apps", true)] {
        let set = set.clone();
        let switch = ui::switch(config::get_bool(&document, key).unwrap_or(fallback), move |on| set(key, on.into()));
        dock.append(&ui::row(label, None, &switch));
    }
    let position = config::get_str(&document, "position").unwrap_or_else(|| "bottom".into());
    let sides = ui::segmented(&POSITIONS, &position, {
        let set = set.clone();
        move |side| set("position", side.into())
    });
    dock.append(&ui::row("Position on screen", None, &sides));
    let autohide = ui::switch(config::get_bool(&document, "autohide").unwrap_or(false), {
        let set = set.clone();
        move |on| set("autohide", on.into())
    });
    dock.append(&ui::row("Hide and show the dock automatically", None, &autohide));
    column.append(&ui::section("Dock", None, &dock));

    let desktop = ui::group();
    let widgets = config::state_path("widgets");
    let shown = config::get_bool(&config::read_path(&widgets), "desktop").unwrap_or(false);
    let switch = ui::switch(shown, {
        let report = report.clone();
        move |on| {
            let mut document = config::read_path(&widgets);
            config::set_in(&mut document, "desktop", on);
            report(config::write_path(&widgets, &document));
        }
    });
    desktop.append(&ui::row("Show widgets on the desktop", Some("The clock and the pinned note, from the notification centre’s Widgets"), &switch));
    let edit = ui::button("Edit widgets…", move |_| {
        let report = report.clone();
        glib::spawn_future_local(async move {
            let opened = async { NotificationsProxy::new(&bus::session().await?).await?.edit_widgets().await };
            report(opened.await.map_err(|error| bus::message(error, "The notification centre (autark-notifyd)")));
        });
    });
    desktop.append(&ui::row("Widgets", Some("Choose them in the notification centre"), &edit));
    column.append(&ui::section("Desktop", None, &desktop));
    column.append(&status);
    page.upcast()
}
