//! Batteries from UPower: the laptop's, then Bluetooth and other wireless devices (BlueZ hands
//! their levels to UPower). Read when the centre opens and every half minute while it's open.

use std::f64::consts::PI;

use gtk::prelude::*;

use crate::shared::Shared;

#[zbus::proxy(interface = "org.freedesktop.UPower", default_service = "org.freedesktop.UPower", default_path = "/org/freedesktop/UPower")]
trait UPower {
    fn enumerate_devices(&self) -> zbus::Result<Vec<zbus::OwnedObjectPath>>;
}

#[zbus::proxy(interface = "org.freedesktop.UPower.Device", default_service = "org.freedesktop.UPower")]
trait Device {
    #[zbus(property, name = "Type")]
    fn kind(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn power_supply(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn model(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn time_to_empty(&self) -> zbus::Result<i64>;
}

const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
/// UPower's `Type` values we use.
const LINE_POWER: u32 = 1;
const BATTERY: u32 = 2;
/// UPower's `State` values.
const CHARGING: u32 = 1;
const DISCHARGING: u32 = 2;
const FULLY_CHARGED: u32 = 4;
/// Below this, in percent, a level shows as low.
const LOW: f64 = 20.0;

#[derive(Clone)]
pub struct Battery {
    pub name: String,
    pub percent: f64,
    pub detail: String,
}

pub async fn read(connection: &zbus::Connection) -> zbus::Result<Vec<Battery>> {
    let mut found = Vec::new();
    let laptop = DeviceProxy::builder(connection).path(DISPLAY_DEVICE).build().await?;
    if laptop.is_present().await? && laptop.kind().await? == BATTERY {
        let percent = laptop.percentage().await?;
        let detail = match laptop.state().await? {
            CHARGING => format!("{percent:.0}%  ·  charging"),
            FULLY_CHARGED => "Charged".to_owned(),
            DISCHARGING => match laptop.time_to_empty().await? {
                seconds if seconds > 0 => format!("{percent:.0}%  ·  {} left", duration(seconds)),
                _ => format!("{percent:.0}%"),
            },
            _ => format!("{percent:.0}%"),
        };
        found.push(Battery { name: "This laptop".to_owned(), percent, detail });
    }
    for path in UPowerProxy::new(connection).await?.enumerate_devices().await? {
        let device = DeviceProxy::builder(connection).path(path).build().await?;
        let kind = device.kind().await?;
        // Devices that power the laptop are already in the display device.
        if device.power_supply().await? || kind == LINE_POWER || !device.is_present().await.unwrap_or(true) {
            continue;
        }
        let percent = device.percentage().await?;
        let model = device.model().await.unwrap_or_default();
        let name = if model.trim().is_empty() { kind_name(kind).to_owned() } else { model.trim().to_owned() };
        let detail = if percent < LOW { format!("{percent:.0}%  ·  low") } else { format!("{percent:.0}%") };
        found.push(Battery { name, percent, detail });
    }
    Ok(found)
}

fn duration(seconds: i64) -> String {
    let minutes = seconds / 60;
    if minutes < 60 { format!("{minutes} min") } else { format!("{} h {} min", minutes / 60, minutes % 60) }
}

/// UPower's names for device types, for devices that don't give a model.
fn kind_name(kind: u32) -> &'static str {
    match kind {
        5 => "Mouse",
        6 => "Keyboard",
        8 => "Phone",
        10 => "Tablet",
        12 => "Game controller",
        13 => "Pen",
        14 => "Touchpad",
        17 => "Headset",
        18 => "Speakers",
        19 => "Headphones",
        26 => "Watch",
        _ => "Device",
    }
}

pub fn view(source: &Shared<Vec<Battery>>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("card");
    card.add_css_class("widget");
    card.set_hexpand(true);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.append(&super::caption("BATTERY"));
    card.append(&rows);
    source.watch(&card, move |_, batteries| {
        while let Some(child) = rows.first_child() {
            rows.remove(&child);
        }
        for battery in batteries {
            rows.append(&row(battery));
        }
    });
    card
}

fn row(battery: &Battery) -> gtk::Box {
    let percent = battery.percent.clamp(0.0, 100.0);
    let ring = gtk::DrawingArea::builder().content_width(30).content_height(30).build();
    ring.add_css_class("ring");
    if percent < LOW {
        ring.add_css_class("low");
    }
    ring.set_draw_func(move |area, cr, width, height| {
        let (x, y) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
        let radius = x.min(y) - 2.0;
        cr.set_line_width(3.5);
        let color = area.color();
        cr.set_source_rgba(f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()), 0.12);
        cr.arc(x, y, radius, 0.0, 2.0 * PI);
        let _ = cr.stroke();
        let palette = &autark_common::theme::look().palette;
        autark_common::theme::set_source(cr, if percent < LOW { &palette.danger } else { &palette.accent }, 1.0);
        cr.arc(x, y, radius, -PI / 2.0, -PI / 2.0 + 2.0 * PI * percent / 100.0);
        let _ = cr.stroke();
    });
    let number = gtk::Label::new(Some(&format!("{percent:.0}")));
    number.add_css_class("ring-number");
    let badge = gtk::Overlay::builder().child(&ring).valign(gtk::Align::Center).build();
    badge.add_overlay(&number);

    let name = gtk::Label::builder().label(&battery.name).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
    name.add_css_class("name");
    let detail = gtk::Label::builder().label(&battery.detail).xalign(0.0).build();
    detail.add_css_class("detail");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_valign(gtk::Align::Center);
    text.set_hexpand(true);
    text.append(&name);
    text.append(&detail);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.append(&badge);
    row.append(&text);
    row
}
