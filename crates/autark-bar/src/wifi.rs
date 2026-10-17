//! Wi-Fi: iwd over D-Bus (Autark's stack), else NetworkManager; hidden without a Wi-Fi device.

mod iwd;
mod network_manager;

use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

use gtk::cairo::{Context, LineCap};
use gtk::glib;
use gtk::prelude::*;

#[derive(Clone, Default, PartialEq)]
pub enum Wifi {
    #[default]
    Unavailable,
    Disconnected,
    Connected { name: String, bars: u8 },
}

pub fn new() -> gtk::Box {
    let wifi = Rc::new(RefCell::new(Wifi::default()));

    let icon = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .valign(gtk::Align::Center)
        .build();
    icon.set_draw_func({
        let wifi = wifi.clone();
        move |area, cr, width, height| draw(area, cr, width.min(height), &wifi.borrow())
    });

    let item = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    item.add_css_class("item");
    item.add_css_class("icon");
    item.set_visible(false);
    item.append(&icon);

    let (updates, received) = async_channel::unbounded();
    glib::spawn_future_local(async move {
        match iwd::watch(updates.clone()).await {
            Ok(true) => {}
            Ok(false) => {
                if let Err(err) = network_manager::watch(updates).await {
                    eprintln!("wifi: NetworkManager: {err}");
                }
            }
            Err(err) => eprintln!("wifi: iwd: {err}"),
        }
    });
    glib::spawn_future_local({
        let item = item.clone();
        async move {
            while let Ok(update) = received.recv().await {
                item.set_visible(update != Wifi::Unavailable);
                item.set_tooltip_text(Some(&match &update {
                    Wifi::Connected { name, .. } => name.clone(),
                    _ => "Wi-Fi: not connected".to_owned(),
                }));
                *wifi.borrow_mut() = update;
                icon.queue_draw();
            }
            // Both watchers have stopped, so what's shown can't be trusted.
            item.set_visible(false);
        }
    });
    item
}

/// The design's Wi-Fi fan: three arcs over a dot, lit from the inside out by signal strength.
fn draw(area: &gtk::DrawingArea, cr: &Context, size: i32, wifi: &Wifi) {
    let bars = match wifi {
        Wifi::Connected { bars, .. } => *bars,
        _ => 0,
    };
    let lit = area.color();
    let dim = lit.with_alpha(lit.alpha() * 0.25);
    let scale = f64::from(size) / 24.0;
    cr.scale(scale, scale);
    cr.set_line_width(2.0);
    cr.set_line_cap(LineCap::Round);

    // Each arc spans from (12 - half, y) to (12 + half, y) on a circle of the given radius.
    for (index, (half, y, radius)) in [(3.5_f64, 15.5_f64, 5.0_f64), (7.0, 12.0, 10.0), (10.0, 8.5, 15.0)].into_iter().enumerate() {
        let centre = y + (radius * radius - half * half).sqrt();
        let start = (y - centre).atan2(-half);
        cr.new_sub_path();
        cr.arc(12.0, centre, radius, start, -PI - start);
        cr.set_source_color(if (index as u8) < bars { &lit } else { &dim });
        let _ = cr.stroke();
    }
    cr.arc(12.0, 19.0, 1.0, 0.0, 2.0 * PI);
    cr.set_source_color(if bars > 0 { &lit } else { &dim });
    let _ = cr.fill();
}
