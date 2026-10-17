//! Display › Night light: edits `nightlight.toml`, then has the daemon reload it; "Turn on now" and the status
//! line go through `org.autark.NightLight1`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;

use crate::{bus, config, ui};

#[zbus::proxy(interface = "org.autark.NightLight1", default_service = "org.autark.NightLight1", default_path = "/org/autark/NightLight1")]
trait NightLight {
    fn set_active(&self, on: bool) -> zbus::Result<()>;
    fn reload(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn active(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn schedule(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn manual(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn night(&self) -> zbus::Result<(i64, i64)>;
    #[zbus(property)]
    fn until(&self) -> zbus::Result<i64>;
}

const DAEMON: &str = "The night light daemon (autark-nightlight)";
const SCHEDULES: [(&str, &str); 3] = [("sun", "Sunset to sunrise"), ("custom", "Custom"), ("off", "Off")];

struct Night {
    proxy: RefCell<Option<NightLightProxy<'static>>>,
    group: gtk::Box,
    status: gtk::Label,
    /// The location row's note, which gives tonight's sunset and sunrise.
    sun_note: RefCell<Option<gtk::Label>>,
    now: RefCell<Option<gtk::Switch>>,
    /// Set while the switch is moved to match the daemon, so that isn't sent back as a request.
    syncing: Cell<bool>,
    debounce: ui::Debounce,
}

pub fn build() -> gtk::Box {
    let group = ui::group();
    let status = gtk::Label::new(None);
    let section = ui::section("Night light", Some(&status), &group);
    let night = Rc::new(Night { proxy: RefCell::default(), group, status, sun_note: RefCell::default(), now: RefCell::default(), syncing: Cell::new(false), debounce: ui::Debounce::default() });
    night.rows();
    glib::spawn_future_local(watch(night));
    section
}

async fn watch(night: Rc<Night>) {
    let connected = async {
        let connection = bus::session().await?;
        // Read fresh each time: the daemon only signals Active, Manual and Temperature changing.
        let proxy = NightLightProxy::builder(&connection).cache_properties(zbus::proxy::CacheProperties::No).build().await?;
        // Fails here, rather than later, when the daemon isn't running.
        proxy.active().await?;
        Ok::<_, zbus::Error>(proxy)
    };
    let proxy = match connected.await {
        Ok(proxy) => proxy,
        Err(error) => {
            night.status.set_label(&bus::message(error, DAEMON));
            night.status.add_css_class("error");
            return;
        }
    };
    let mut changes = proxy.receive_active_changed().await;
    *night.proxy.borrow_mut() = Some(proxy);
    night.refresh().await;
    while changes.next().await.is_some() {
        night.refresh().await;
    }
}

fn clock(unix: i64) -> String {
    glib::DateTime::from_unix_local(unix).and_then(|time| time.format("%H:%M")).map(|text| text.to_string()).unwrap_or_default()
}

impl Night {
    /// Reads the daemon's state into the status line, the sun times and the switch.
    async fn refresh(self: &Rc<Self>) {
        let Some(proxy) = self.proxy.borrow().clone() else { return };
        let (Ok(active), Ok(schedule), Ok(until), Ok(night)) = (proxy.active().await, proxy.schedule().await, proxy.until().await, proxy.night().await) else { return };
        let sun = schedule == "sun";
        let status = match (active, until) {
            (true, 0) => "On until you turn it off".to_owned(),
            (true, until) if sun && until == night.1 => format!("On until sunrise, {}", clock(until)),
            (true, until) => format!("On until {}", clock(until)),
            (false, 0) => "Off".to_owned(),
            (false, until) if sun && until == night.0 => format!("Comes on at sunset, {}", clock(until)),
            (false, until) => format!("Comes on at {}", clock(until)),
        };
        self.status.set_label(&status);
        self.status.remove_css_class("error");
        if let Some(note) = self.sun_note.borrow().as_ref() {
            note.set_label(&if sun && night != (0, 0) {
                format!("Stays on this laptop. Sunset {}, sunrise {}", clock(night.0), clock(night.1))
            } else {
                "Stays on this laptop. Until both are set, the custom times are used".to_owned()
            });
        }
        if let Some(switch) = self.now.borrow().as_ref()
            && switch.is_active() != active
        {
            self.syncing.set(true);
            switch.set_active(active);
            self.syncing.set(false);
        }
    }

    fn rows(self: &Rc<Self>) {
        ui::clear(&self.group);
        let document = config::read("nightlight");
        let schedule = config::get_str(&document, "schedule").unwrap_or_else(|| "sun".into());

        let night = self.clone();
        let segmented = ui::segmented(&SCHEDULES, &schedule, move |value| {
            let value = value.to_owned();
            night.write(move |document| config::set_in(document, "schedule", value));
            // Show the rows that go with it, after this button's own handler is done.
            let night = night.clone();
            glib::idle_add_local_once(move || night.rows());
        });
        self.group.append(&ui::row("Schedule", None, &segmented));

        *self.sun_note.borrow_mut() = None;
        match schedule.as_str() {
            "sun" => {
                let fields = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                for (key, limit) in [("latitude", 90.0), ("longitude", 180.0)] {
                    let text = config::get_float(&document, key).map(|value| value.to_string()).unwrap_or_default();
                    let night = self.clone();
                    let entry = ui::field(&text, 7, move |text| {
                        let text = text.trim().replace(',', ".");
                        if text.is_empty() {
                            night.write(move |document| config::remove_in(document, key));
                            return true;
                        }
                        let Some(value) = text.parse::<f64>().ok().filter(|value| value.abs() <= limit) else { return false };
                        night.write(move |document| config::set_in(document, key, value));
                        true
                    });
                    entry.set_placeholder_text(Some(if key == "latitude" { "Latitude" } else { "Longitude" }));
                    entry.update_property(&[gtk::accessible::Property::Label(if key == "latitude" { "Latitude" } else { "Longitude" })]);
                    fields.append(&entry);
                }
                let row = ui::row("Location", Some("Stays on this laptop"), &fields);
                // The note under "Location" gets the sun times once the daemon gives them.
                *self.sun_note.borrow_mut() = row.first_child().and_then(|label| label.last_child()).and_downcast::<gtk::Label>();
                self.group.append(&row);
            }
            "custom" => {
                let times = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                for (index, (key, fallback)) in [("from", "22:00"), ("to", "07:00")].into_iter().enumerate() {
                    if index == 1 {
                        times.append(&ui::note("to"));
                    }
                    let text = config::get_str(&document, key).unwrap_or_else(|| fallback.into());
                    let night = self.clone();
                    let entry = ui::field(&text, 5, move |text| {
                        let Some(time) = parse_time(text) else { return false };
                        night.write(move |document| config::set_in(document, key, time));
                        true
                    });
                    entry.update_property(&[gtk::accessible::Property::Label(if key == "from" { "From" } else { "To" })]);
                    times.append(&entry);
                }
                self.group.append(&ui::row("From", None, &times));
            }
            _ => {}
        }

        let temperature = config::get_int(&document, "temperature").unwrap_or(4000).clamp(2500, 6000);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let value = gtk::Label::new(Some(&format!("{temperature} K")));
        value.add_css_class("value");
        value.set_width_chars(6);
        value.set_xalign(1.0);
        let night = self.clone();
        let slider = ui::slider(2500.0, 6000.0, 100.0, temperature as f64, "More", "Less", {
            let value = value.clone();
            move |kelvin| {
                let kelvin = ((kelvin / 100.0).round() * 100.0) as i64;
                value.set_label(&format!("{kelvin} K"));
                let night = night.clone();
                night.clone().debounce.run(Duration::from_millis(300), move || night.write(move |document| config::set_in(document, "temperature", kelvin)));
            }
        });
        line.append(&slider);
        line.append(&value);
        self.group.append(&ui::row("Warmth", None, &line));

        let note = if schedule == "off" { "Stays on until you turn it off" } else { "Until the schedule next changes" };
        let night = self.clone();
        let switch = ui::switch(false, move |on| {
            if night.syncing.get() {
                return;
            }
            let Some(proxy) = night.proxy.borrow().clone() else { return };
            let night = night.clone();
            glib::spawn_future_local(async move {
                if let Err(error) = proxy.set_active(on).await {
                    night.status.set_label(&bus::message(error, DAEMON));
                }
                night.refresh().await;
            });
        });
        self.group.append(&ui::row("Turn on now", Some(note), &switch));
        *self.now.borrow_mut() = Some(switch);
        let night = self.clone();
        glib::spawn_future_local(async move { night.refresh().await });
    }

    /// Changes `nightlight.toml` and has the daemon read it again, keeping a by-hand on or off as it was.
    fn write(self: &Rc<Self>, change: impl FnOnce(&mut toml_edit::DocumentMut)) {
        if let Err(error) = config::edit("nightlight", change) {
            self.status.set_label(&error);
            self.status.add_css_class("error");
            return;
        }
        let Some(proxy) = self.proxy.borrow().clone() else { return };
        let night = self.clone();
        glib::spawn_future_local(async move {
            let by_hand = match proxy.manual().await {
                Ok(true) => proxy.active().await.ok(),
                _ => None,
            };
            if let Err(error) = proxy.reload().await {
                night.status.set_label(&bus::message(error, DAEMON));
                return;
            }
            if let Some(on) = by_hand {
                let _ = proxy.set_active(on).await;
            }
            night.refresh().await;
        });
    }
}

/// "7:30" or "07:30" as "07:30".
fn parse_time(text: &str) -> Option<String> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then(|| format!("{hours:02}:{minutes:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_tidied() {
        assert_eq!(parse_time("7:30").as_deref(), Some("07:30"));
        assert_eq!(parse_time(" 22:00 ").as_deref(), Some("22:00"));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("noon"), None);
    }
}
