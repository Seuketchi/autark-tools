//! The clock, redrawn on the minute (or the second, if the format shows seconds) and after waking.

use std::time::Duration;

use chrono::Local;
use chrono::format::{Item, StrftimeItems};
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;

use crate::shared::Shared;

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

/// One clock for every bar: the formatted time, updated on the minute (or second) and after waking.
pub fn source(format: &str) -> Shared<String> {
    let format = if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        eprintln!("bar.toml: clock_format {format:?} is not a valid format; using the default");
        DEFAULT_FORMAT
    } else {
        format
    };
    let seconds = ["%S", "%T", "%X", "%r", "%s"].iter().any(|spec| format.contains(spec));

    let shared = Shared::new(String::new());
    tick(shared.clone(), format.to_owned(), if seconds { 1 } else { 60 });
    glib::spawn_future_local(refresh_after_sleep(shared.clone(), format.to_owned()));
    shared
}

pub fn view(source: &Shared<String>) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("item");
    label.add_css_class("clock");
    source.watch(&label, |label, time| label.set_label(time));
    label
}

fn show(shared: &Shared<String>, format: &str) {
    shared.set(Local::now().format(format).to_string());
}

fn tick(shared: Shared<String>, format: String, interval: i64) {
    show(&shared, &format);
    let now = Local::now();
    let wait = Duration::from_secs((interval - now.timestamp() % interval) as u64)
        .saturating_sub(Duration::from_nanos(now.timestamp_subsec_nanos().into()));
    glib::timeout_add_local_once(wait, move || tick(shared, format, interval));
}

/// Timers stop while the laptop sleeps, so the time is shown again as soon as it wakes.
async fn refresh_after_sleep(shared: Shared<String>, format: String) {
    let watch = async {
        let connection = zbus::Connection::system().await?;
        let mut signals = LoginProxy::new(&connection).await?.receive_prepare_for_sleep().await?;
        while let Some(signal) = signals.next().await {
            if !signal.args()?.start {
                show(&shared, &format);
            }
        }
        zbus::Result::Ok(())
    };
    if let Err(err) = watch.await {
        eprintln!("clock: not watching for sleep: {err}");
    }
}
