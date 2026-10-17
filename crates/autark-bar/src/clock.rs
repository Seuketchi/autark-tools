//! The clock, redrawn on the minute (or the second, if the format shows seconds) and after waking.

use std::time::Duration;

use chrono::Local;
use chrono::format::{Item, StrftimeItems};
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;

pub const DEFAULT_FORMAT: &str = "%a %-d %b  %H:%M";

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

pub fn new(format: &str) -> gtk::Label {
    let format = if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        eprintln!("bar.toml: clock_format {format:?} is not a valid format; using the default");
        DEFAULT_FORMAT
    } else {
        format
    };
    let seconds = ["%S", "%T", "%X", "%r", "%s"].iter().any(|spec| format.contains(spec));

    let label = gtk::Label::new(None);
    label.add_css_class("item");
    label.add_css_class("clock");
    tick(label.clone(), format.to_owned(), if seconds { 1 } else { 60 });
    glib::spawn_future_local(refresh_after_sleep(label.clone(), format.to_owned()));
    label
}

fn show(label: &gtk::Label, format: &str) {
    label.set_label(&Local::now().format(format).to_string());
}

fn tick(label: gtk::Label, format: String, interval: i64) {
    show(&label, &format);
    let now = Local::now();
    let wait = Duration::from_secs((interval - now.timestamp() % interval) as u64)
        .saturating_sub(Duration::from_nanos(now.timestamp_subsec_nanos().into()));
    glib::timeout_add_local_once(wait, move || tick(label, format, interval));
}

/// Timers stop while the laptop sleeps, so the time is shown again as soon as it wakes.
async fn refresh_after_sleep(label: gtk::Label, format: String) {
    let watch = async {
        let connection = zbus::Connection::system().await?;
        let mut signals = LoginProxy::new(&connection).await?.receive_prepare_for_sleep().await?;
        while let Some(signal) = signals.next().await {
            if !signal.args()?.start {
                show(&label, &format);
            }
        }
        zbus::Result::Ok(())
    };
    if let Err(err) = watch.await {
        eprintln!("clock: not watching for sleep: {err}");
    }
}
