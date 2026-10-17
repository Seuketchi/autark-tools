//! Power: the battery from UPower (charge, time left, health), and the timers and warning in `power.toml`,
//! which `autark-power` watches. Showing the percentage in the top bar is `bar.toml`'s, which the bar watches.

use std::cell::Cell;
use std::rc::Rc;

use autark_common::theme;
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;

use crate::{bus, config, ui};

#[zbus::proxy(interface = "org.freedesktop.UPower", default_service = "org.freedesktop.UPower", default_path = "/org/freedesktop/UPower")]
trait UPower {
    fn enumerate_devices(&self) -> zbus::Result<Vec<zbus::OwnedObjectPath>>;
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(interface = "org.freedesktop.UPower.Device", default_service = "org.freedesktop.UPower", default_path = "/org/freedesktop/UPower/devices/DisplayDevice")]
trait Device {
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn time_to_empty(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn time_to_full(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property, name = "Type")]
    fn kind(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn power_supply(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn energy_full(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn energy_full_design(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn charge_cycles(&self) -> zbus::Result<i32>;
}

const UPOWER: &str = "UPower";
/// UPower's device states and the battery type.
const CHARGING: u32 = 1;
const DISCHARGING: u32 = 2;
const FULL: u32 = 4;
const BATTERY: u32 = 2;

const BATTERY_SCREEN: [(&str, &str); 4] = [("2", "2 minutes"), ("5", "5 minutes"), ("10", "10 minutes"), ("0", "Never")];
const BATTERY_SLEEP: [(&str, &str); 4] = [("10", "10 minutes"), ("15", "15 minutes"), ("30", "30 minutes"), ("0", "Never")];
const PLUGGED_SCREEN: [(&str, &str); 4] = [("5", "5 minutes"), ("10", "10 minutes"), ("30", "30 minutes"), ("0", "Never")];
const PLUGGED_SLEEP: [(&str, &str); 3] = [("30", "30 minutes"), ("60", "1 hour"), ("0", "Never")];
const LOW: [(&str, &str); 3] = [("10", "At 10%"), ("15", "At 15%"), ("20", "At 20%")];

struct Battery {
    group: gtk::Box,
    level: gtk::DrawingArea,
    shown: Rc<Cell<f64>>,
    percent: gtk::Label,
    status: gtk::Label,
    health: gtk::Label,
    on_battery: gtk::Box,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Power", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let report = {
        let status = status.clone();
        move |result: Result<(), String>| {
            status.set_visible(result.is_err());
            if let Err(error) = result {
                status.set_label(&error);
            }
        }
    };

    let shown = Rc::new(Cell::new(0.0));
    let level = gtk::DrawingArea::builder().content_width(50).content_height(24).valign(gtk::Align::Center).build();
    level.set_draw_func({
        let shown = shown.clone();
        move |_, cr, width, height| draw_battery(cr, f64::from(width), f64::from(height), shown.get())
    });
    theme::redraw_on_change(&level);
    let percent = gtk::Label::new(Some("—"));
    percent.add_css_class("battery-percent");
    percent.set_xalign(0.0);
    let charge = ui::note("");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&percent);
    text.append(&charge);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    top.add_css_class("row");
    top.add_css_class("battery");
    top.append(&level);
    top.append(&text);
    let group = ui::group();
    group.append(&top);
    let health = gtk::Label::new(Some("—"));
    health.add_css_class("value");
    group.append(&ui::row("Battery health", None, &health));
    column.append(&group);

    let document = config::read("power");
    let timers = |section: &'static str, screen: &[(&str, &str)], sleep: &[(&str, &str)], defaults: (i64, i64)| {
        let group = ui::group();
        for (key, label, options, fallback) in [("screen_off", "Turn the screen off after", screen, defaults.0), ("sleep", "Sleep after", sleep, defaults.1)] {
            let key = format!("{section}.{key}");
            let current = config::get_int(&document, &key).unwrap_or(fallback);
            let report = report.clone();
            let dropdown = minutes_choice(options, current, move |minutes| report(config::edit("power", |document| config::set_in(document, &key, minutes))));
            let place = if section == "battery" { "on battery" } else { "plugged in" };
            dropdown.update_property(&[gtk::accessible::Property::Label(&format!("{label}, {place}"))]);
            group.append(&ui::row(label, None, &dropdown));
        }
        group
    };
    let on_battery = ui::section("On battery", None, &timers("battery", &BATTERY_SCREEN, &BATTERY_SLEEP, (2, 10)));
    column.append(&on_battery);
    column.append(&ui::section("Plugged in", None, &timers("plugged_in", &PLUGGED_SCREEN, &PLUGGED_SLEEP, (5, 30))));

    let more = ui::group();
    let bar = config::get_bool(&config::read("bar"), "battery_percent").unwrap_or(true);
    let switch = ui::switch(bar, {
        let report = report.clone();
        move |on| report(config::edit("bar", |document| config::set_in(document, "battery_percent", on)))
    });
    switch.update_property(&[gtk::accessible::Property::Label("Show the battery percentage in the top bar")]);
    more.append(&ui::row("Show the percentage in the top bar", None, &switch));
    let low = config::get_int(&document, "low_battery").unwrap_or(10);
    let warn = {
        let report = report.clone();
        let mut options: Vec<(String, String)> = LOW.iter().map(|(value, label)| (value.to_string(), label.to_string())).collect();
        if !options.iter().any(|(value, _)| *value == low.to_string()) {
            options.push((low.to_string(), if low == 0 { "Never".into() } else { format!("At {low}%") }));
        }
        let options: Vec<(&str, &str)> = options.iter().map(|(value, label)| (value.as_str(), label.as_str())).collect();
        ui::choice(&options, &low.to_string(), move |value| {
            let percent: i64 = value.parse().unwrap_or(10);
            report(config::edit("power", |document| config::set_in(document, "low_battery", percent)));
        })
    };
    warn.update_property(&[gtk::accessible::Property::Label("Warn when the battery is low")]);
    more.append(&ui::row("Warn when the battery is low", None, &warn));
    let lid = gtk::Label::new(Some("Sleeps, unless a display is connected"));
    lid.add_css_class("value");
    more.append(&ui::row("Closing the lid", None, &lid));
    column.append(&more);
    column.append(&status);

    let battery = Rc::new(Battery { group, level, shown, percent, status: charge, health, on_battery });
    glib::spawn_future_local(watch(battery));
    page.upcast()
}

/// A drop-down of minutes, with the file's own value added when it isn't one of the choices.
fn minutes_choice(options: &[(&str, &str)], current: i64, changed: impl Fn(i64) + 'static) -> gtk::DropDown {
    let mut owned: Vec<(String, String)> = options.iter().map(|(value, label)| (value.to_string(), label.to_string())).collect();
    if !owned.iter().any(|(value, _)| *value == current.to_string()) {
        owned.push((current.to_string(), duration(current * 60)));
    }
    let options: Vec<(&str, &str)> = owned.iter().map(|(value, label)| (value.as_str(), label.as_str())).collect();
    ui::choice(&options, &current.to_string(), move |value| changed(value.parse().unwrap_or(0)))
}

async fn watch(battery: Rc<Battery>) {
    let connected = async {
        let connection = bus::system().await?;
        let upower = UPowerProxy::new(&connection).await?;
        let display = DeviceProxy::new(&connection).await?;
        // The laptop's own battery, for its health; the display device is the sum of every battery.
        let mut own = None;
        for path in upower.enumerate_devices().await? {
            let device = DeviceProxy::builder(&connection).path(path).build().await?;
            if device.kind().await? == BATTERY && device.power_supply().await.unwrap_or(false) {
                own = Some(device);
                break;
            }
        }
        Ok::<_, zbus::Error>((upower, display, own))
    };
    let (upower, display, own) = match connected.await {
        Ok(found) => found,
        Err(error) => {
            battery.unavailable(&bus::message(error, UPOWER));
            return;
        }
    };
    let mut changes = futures_util::stream::select_all([
        display.receive_percentage_changed().await.map(|_| ()).boxed_local(),
        display.receive_state_changed().await.map(|_| ()).boxed_local(),
        display.receive_time_to_empty_changed().await.map(|_| ()).boxed_local(),
        display.receive_time_to_full_changed().await.map(|_| ()).boxed_local(),
        upower.receive_on_battery_changed().await.map(|_| ()).boxed_local(),
    ]);
    loop {
        battery.show(&display, own.as_ref()).await;
        if changes.next().await.is_none() {
            return;
        }
    }
}

impl Battery {
    async fn show(&self, display: &DeviceProxy<'_>, own: Option<&DeviceProxy<'_>>) {
        if !display.is_present().await.unwrap_or(false) {
            self.unavailable("This computer has no battery.");
            return;
        }
        let percentage = display.percentage().await.unwrap_or(0.0);
        self.shown.set(percentage / 100.0);
        self.level.queue_draw();
        self.percent.set_label(&format!("{percentage:.0}%"));
        let state = display.state().await.unwrap_or(0);
        let status = match state {
            DISCHARGING => match display.time_to_empty().await.unwrap_or(0) {
                0 => "On battery".to_owned(),
                seconds => format!("On battery · about {} left", duration(seconds)),
            },
            CHARGING => match display.time_to_full().await.unwrap_or(0) {
                0 => "Charging".to_owned(),
                seconds => format!("Charging · full in {}", duration(seconds)),
            },
            FULL => "Fully charged".to_owned(),
            _ => "Plugged in, not charging".to_owned(),
        };
        self.status.set_label(&status);
        let health = match own {
            Some(own) => {
                let (full, design) = (own.energy_full().await.unwrap_or(0.0), own.energy_full_design().await.unwrap_or(0.0));
                let cycles = own.charge_cycles().await.unwrap_or(-1);
                match (design > 0.0, cycles > 0) {
                    (true, true) => format!("{:.0}% of its design capacity · {cycles} cycles", (full / design * 100.0).min(100.0)),
                    (true, false) => format!("{:.0}% of its design capacity", (full / design * 100.0).min(100.0)),
                    _ => "Not reported".to_owned(),
                }
            }
            None => "Not reported".to_owned(),
        };
        self.health.set_label(&health);
    }

    fn unavailable(&self, text: &str) {
        ui::clear(&self.group);
        let label = ui::note(text);
        label.add_css_class("unavailable");
        label.set_wrap(true);
        self.group.append(&label);
        self.on_battery.set_visible(false);
    }
}

/// "5 h 10 min", "45 min".
fn duration(seconds: i64) -> String {
    let minutes = (seconds + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (1, 0) => "1 hour".to_owned(),
        (hours, 0) => format!("{hours} hours"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// The design's battery: an outline with a nub, filled in the accent to the charge.
fn draw_battery(cr: &gtk::cairo::Context, width: f64, height: f64, charge: f64) {
    let look = theme::look();
    let body = width - 4.0;
    theme::set_source(cr, &look.palette.text, 1.0);
    cr.set_line_width(2.0);
    rounded(cr, 1.0, 1.0, body - 2.0, height - 2.0, 5.0);
    cr.stroke().ok();
    rounded(cr, body, height / 2.0 - 4.0, 3.0, 8.0, 1.0);
    cr.fill().ok();
    theme::set_source(cr, &look.palette.accent, 1.0);
    let inner = body - 8.0;
    rounded(cr, 4.0, 4.0, (inner * charge.clamp(0.0, 1.0)).max(1.0), height - 8.0, 2.5);
    cr.fill().ok();
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, PI + FRAC_PI_2);
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(18_600), "5 h 10 min");
        assert_eq!(duration(2_700), "45 min");
        assert_eq!(duration(3_600), "1 hour");
        assert_eq!(duration(7_200), "2 hours");
    }
}
