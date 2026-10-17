//! Resources from `autark-stats`: watched every two seconds while the centre shows widgets, and
//! not at all otherwise.

use std::rc::Rc;
use std::time::Duration;

use autark_common::stats::Snapshot;
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;

use crate::shared::Shared;

#[zbus::proxy(interface = "org.autark.Stats1", default_service = "org.autark.Stats1", default_path = "/org/autark/Stats1")]
pub trait Stats {
    fn watch(&self, interval_ms: u32) -> zbus::Result<()>;
    fn unwatch(&self) -> zbus::Result<()>;
    fn snapshot(&self) -> zbus::Result<String>;
    #[zbus(signal)]
    fn updated(&self, snapshot: &str) -> zbus::Result<()>;
}

#[derive(Clone)]
pub enum Feed {
    Waiting,
    Reading(Rc<Snapshot>),
    /// The service isn't there.
    Unavailable,
}

const INTERVAL_MS: u32 = 2000;

/// Watches until `stop` fires, then unwatches. Retries every five seconds if the service is gone.
pub async fn follow(feed: Shared<Feed>, stop: async_channel::Receiver<()>) {
    let watching = async {
        loop {
            let result = async {
                let connection = zbus::Connection::session().await?;
                let proxy = StatsProxy::new(&connection).await?;
                let mut updates = proxy.receive_updated().await?;
                proxy.watch(INTERVAL_MS).await?;
                // Unwatches on the way out, however this future ends.
                let _unwatch = Unwatch(proxy.clone());
                while let Some(update) = updates.next().await {
                    match serde_json::from_str::<Snapshot>(update.args()?.snapshot) {
                        Ok(snapshot) => feed.set(Feed::Reading(Rc::new(snapshot))),
                        Err(err) => eprintln!("notifyd: a stats update didn't parse: {err}"),
                    }
                }
                zbus::Result::Ok(())
            };
            if result.await.is_err() {
                feed.set(Feed::Unavailable);
            }
            glib::timeout_future(Duration::from_secs(5)).await;
        }
    };
    let stopped = stop.recv();
    futures_util::future::select(std::pin::pin!(watching), std::pin::pin!(stopped)).await;
}

/// Sends `Unwatch` when dropped. Leaving the bus would do too, but notifyd never leaves it.
struct Unwatch(StatsProxy<'static>);

impl Drop for Unwatch {
    fn drop(&mut self) {
        let proxy = self.0.clone();
        glib::spawn_future_local(async move {
            let _ = proxy.unwatch().await;
        });
    }
}

pub fn view(feed: &Shared<Feed>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 9);
    card.add_css_class("card");
    card.add_css_class("widget");
    card.set_hexpand(true);
    card.append(&super::caption("RESOURCES"));
    let (cpu, cpu_meter) = row(&card, "CPU", "cpu");
    let (memory, memory_meter) = row(&card, "Memory", "memory");
    let (power, power_meter) = row(&card, "Power", "power");
    let rows: Vec<gtk::Widget> = std::iter::successors(card.first_child(), |child| child.next_sibling()).skip(1).collect();
    let note = gtk::Label::builder().xalign(0.0).wrap(true).build();
    note.add_css_class("detail");
    card.append(&note);

    feed.watch(&card, move |_, feed| {
        let reading = match feed {
            Feed::Reading(snapshot) => Some(snapshot),
            _ => None,
        };
        note.set_visible(!matches!(feed, Feed::Reading(_)));
        note.set_label(match feed {
            Feed::Unavailable => "autark-stats isn't running",
            _ => "Reading…",
        });
        for row in &rows {
            row.set_visible(reading.is_some());
        }
        let Some(snapshot) = reading else { return };
        // Without RAPL or a discharging battery there's no power figure to show.
        rows[2].set_visible(snapshot.power.is_some());
        cpu.set_label(&format!("{:.0}%", snapshot.cpu.total));
        set_level(&cpu_meter, snapshot.cpu.total / 100.0);
        memory.set_label(&gigabytes(snapshot.memory.used));
        set_level(&memory_meter, if snapshot.memory.total > 0 { snapshot.memory.used as f64 / snapshot.memory.total as f64 } else { 0.0 });
        let watts = snapshot.power.as_ref().map(|power| power.watts);
        power.set_label(&format!("{:.1} W", watts.unwrap_or(0.0)));
        // Twenty watts fills the bar: about what a laptop draws flat out.
        set_level(&power_meter, watts.unwrap_or(0.0) / 20.0);
    });
    card
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// A labelled row and its meter under it.
fn row(card: &gtk::Box, name: &str, kind: &str) -> (gtk::Label, gtk::DrawingArea) {
    let label = gtk::Label::builder().label(name).xalign(0.0).hexpand(true).build();
    let value = gtk::Label::builder().xalign(1.0).build();
    value.add_css_class("value");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    line.append(&label);
    line.append(&value);
    let meter = gtk::DrawingArea::builder().content_height(5).build();
    meter.add_css_class("meter");
    meter.add_css_class(kind);
    set_level(&meter, 0.0);
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row.append(&line);
    row.append(&meter);
    card.append(&row);
    (value, meter)
}

fn set_level(meter: &gtk::DrawingArea, level: f64) {
    let level = level.clamp(0.0, 1.0);
    meter.set_draw_func(move |area, cr, width, height| {
        let (width, height) = (f64::from(width), f64::from(height));
        let rounded = |cr: &gtk::cairo::Context, width: f64| {
            let radius = height / 2.0;
            cr.new_sub_path();
            cr.arc(width - radius, radius, radius, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            cr.arc(radius, radius, radius, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
            cr.close_path();
        };
        autark_common::theme::set_source(cr, &autark_common::theme::look().palette.text, 0.08);
        rounded(cr, width);
        let _ = cr.fill();
        if level > 0.0 {
            cr.set_source_color(&area.color());
            rounded(cr, (width * level).max(height));
            let _ = cr.fill();
        }
    });
}
