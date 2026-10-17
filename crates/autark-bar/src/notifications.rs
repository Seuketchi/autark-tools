//! The notification centre button: opens autark-notifyd's centre. Hidden while notifyd isn't
//! running.

use futures_util::StreamExt;
use gtk::cairo::LineCap;
use gtk::glib;
use gtk::prelude::*;

use crate::shared::Shared;

const SERVICE: &str = "org.autark.Notifications1";

#[zbus::proxy(
    interface = "org.autark.Notifications1",
    default_service = "org.autark.Notifications1",
    default_path = "/org/autark/Notifications"
)]
trait Control {
    fn toggle(&self) -> zbus::Result<()>;
}

/// Whether notifyd is running, watched once for every bar.
pub fn source() -> Shared<bool> {
    let shared = Shared::new(false);
    glib::spawn_future_local({
        let shared = shared.clone();
        async move {
            let watch = async {
                let connection = zbus::Connection::session().await?;
                let bus = zbus::fdo::DBusProxy::new(&connection).await?;
                let mut changes = bus.receive_name_owner_changed().await?;
                shared.set(bus.name_has_owner(SERVICE.try_into()?).await?);
                while let Some(change) = changes.next().await {
                    let args = change.args()?;
                    if args.name.as_str() == SERVICE {
                        shared.set(args.new_owner.is_some());
                    }
                }
                zbus::Result::Ok(())
            };
            if let Err(err) = watch.await {
                eprintln!("notifications: {err}");
            }
            shared.set(false);
        }
    });
    shared
}

pub fn view(source: &Shared<bool>) -> gtk::Button {
    let icon = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .valign(gtk::Align::Center)
        .build();
    // The design's icon: a rounded panel with its right-hand column marked off.
    icon.set_draw_func(|area, cr, width, height| {
        let scale = f64::from(width.min(height)) / 24.0;
        cr.scale(scale, scale);
        cr.set_source_color(&area.color());
        cr.set_line_width(2.0);
        cr.set_line_cap(LineCap::Round);
        rounded_rect(cr, 3.0, 4.0, 18.0, 16.0, 3.0);
        cr.move_to(14.0, 4.0);
        cr.line_to(14.0, 20.0);
        let _ = cr.stroke();
    });
    let button = gtk::Button::builder().child(&icon).tooltip_text("Notification centre").build();
    button.add_css_class("item");
    button.add_css_class("icon");
    button.connect_clicked(|_| {
        glib::spawn_future_local(async {
            let toggle = async { ControlProxy::new(&zbus::Connection::session().await?).await?.toggle().await };
            if let Err(err) = toggle.await {
                eprintln!("notifications: opening the centre: {err}");
            }
        });
    });
    source.watch(&button, |button, running| button.set_visible(*running));
    button
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}
