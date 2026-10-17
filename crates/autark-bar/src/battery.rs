//! Battery level and charging state from UPower's display device; hidden on machines without one.

use std::cell::Cell;
use std::rc::Rc;

use futures_util::StreamExt;
use gtk::cairo::{Context, Operator};
use gtk::glib;
use gtk::prelude::*;

#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower/devices/DisplayDevice"
)]
trait Device {
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
}

/// UPower's `State` values.
const CHARGING: u32 = 1;
const FULLY_CHARGED: u32 = 4;

#[derive(Clone, Copy, Default)]
struct Level {
    percent: f64,
    state: u32,
}

pub fn new(show_percent: bool) -> gtk::Box {
    let level = Rc::new(Cell::new(Level::default()));

    let label = gtk::Label::new(None);
    label.set_visible(show_percent);

    let icon = gtk::DrawingArea::builder()
        .content_width(24)
        .content_height(14)
        .valign(gtk::Align::Center)
        .build();
    icon.set_draw_func({
        let level = level.clone();
        move |area, cr, _, _| draw(area, cr, level.get())
    });

    let item = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    item.add_css_class("item");
    item.set_visible(false);
    item.append(&label);
    item.append(&icon);
    glib::spawn_future_local(watch(item.clone(), label, icon, level));
    item
}

async fn watch(item: gtk::Box, label: gtk::Label, icon: gtk::DrawingArea, level: Rc<Cell<Level>>) {
    let update = |current: Level| {
        level.set(current);
        label.set_label(&format!("{:.0}%", current.percent));
        item.set_tooltip_text(Some(&match current.state {
            CHARGING => format!("{:.0}%, charging", current.percent),
            FULLY_CHARGED => "Fully charged".to_owned(),
            _ => format!("{:.0}%", current.percent),
        }));
        icon.queue_draw();
    };
    let result = async {
        let connection = zbus::Connection::system().await?;
        let device = DeviceProxy::new(&connection).await?;
        if !device.is_present().await? {
            return Ok(());
        }
        let mut current = Level {
            percent: device.percentage().await?,
            state: device.state().await?,
        };
        update(current);
        item.set_visible(true);

        let mut percent = device.receive_percentage_changed().await.fuse();
        let mut state = device.receive_state_changed().await.fuse();
        loop {
            futures_util::select! {
                change = percent.next() => match change {
                    Some(change) => current.percent = change.get().await?,
                    None => break,
                },
                change = state.next() => match change {
                    Some(change) => current.state = change.get().await?,
                    None => break,
                },
            }
            update(current);
        }
        zbus::Result::Ok(())
    };
    if let Err(err) = result.await {
        eprintln!("battery: {err}");
        item.set_visible(false);
    }
}

/// The design's battery: a rounded body filled to the level, a terminal nub, and a bolt while charging.
fn draw(area: &gtk::DrawingArea, cr: &Context, level: Level) {
    cr.set_source_color(&area.color());
    cr.set_line_width(1.5);
    rounded_rect(cr, 1.0, 1.0, 21.0, 12.0, 3.0);
    let _ = cr.stroke();
    cr.move_to(24.0, 5.0);
    cr.line_to(24.0, 9.0);
    let _ = cr.stroke();

    let fill = 17.0 * (level.percent / 100.0).clamp(0.0, 1.0);
    if fill > 0.0 {
        rounded_rect(cr, 3.0, 3.0, fill, 8.0, 1.5_f64.min(fill / 2.0));
        let _ = cr.fill();
    }

    if level.state == CHARGING {
        let bolt = |cr: &Context| {
            cr.move_to(13.0, 2.0);
            cr.line_to(8.0, 7.5);
            cr.line_to(11.0, 7.5);
            cr.line_to(10.0, 12.0);
            cr.line_to(15.0, 6.5);
            cr.line_to(12.0, 6.5);
            cr.close_path();
        };
        // Cut the bolt out of the fill with a margin, then draw it, so it reads on any level.
        cr.set_operator(Operator::Clear);
        bolt(cr);
        cr.set_line_width(2.0);
        let _ = cr.stroke_preserve();
        cr.set_operator(Operator::Over);
        let _ = cr.fill();
    }
}

fn rounded_rect(cr: &Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}
