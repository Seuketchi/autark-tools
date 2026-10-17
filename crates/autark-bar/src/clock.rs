//! The clock, redrawn on the minute (or the second, if the format shows seconds) and after waking.

use std::time::Duration;

use chrono::Local;
use chrono::format::{Item, StrftimeItems};
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;

use crate::shared::Shared;

pub const DEFAULT_FORMAT: &str = "%a %-d %b  %H:%M";

#[zbus::proxy(interface = "org.freedesktop.login1.Manager", default_service = "org.freedesktop.login1", default_path = "/org/freedesktop/login1")]
trait Login {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

thread_local! {
    static FORMAT: std::cell::RefCell<String> = std::cell::RefCell::new(DEFAULT_FORMAT.to_owned());
    /// Bumped by each format, so the ticks timed for the one before stop.
    static TICKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// One clock for every bar: the formatted time, updated on the minute (or second) and after waking.
pub fn source(format: &str) -> Shared<String> {
    let shared = Shared::new(String::new());
    set_format(&shared, format);
    glib::spawn_future_local(refresh_after_sleep(shared.clone()));
    shared
}

/// Changes the format, as when `bar.toml` is edited.
pub fn set_format(shared: &Shared<String>, format: &str) {
    let format = if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        eprintln!("bar.toml: clock_format {format:?} is not a valid format; using the default");
        DEFAULT_FORMAT
    } else {
        format
    };
    FORMAT.with_borrow_mut(|current| *current = format.to_owned());
    let generation = TICKS.get().wrapping_add(1);
    TICKS.set(generation);
    tick(shared.clone(), generation);
}

pub fn view(source: &Shared<String>) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("item");
    label.add_css_class("clock");
    source.watch(&label, |label, time| label.set_label(time));
    label
}

fn show(shared: &Shared<String>) {
    FORMAT.with_borrow(|format| shared.set(Local::now().format(format).to_string()));
}

fn tick(shared: Shared<String>, generation: u32) {
    if TICKS.get() != generation {
        return;
    }
    show(&shared);
    let seconds = FORMAT.with_borrow(|format| ["%S", "%T", "%X", "%r", "%s"].iter().any(|spec| format.contains(spec)));
    let interval = if seconds { 1 } else { 60 };
    let now = Local::now();
    let wait = Duration::from_secs((interval - now.timestamp() % interval) as u64).saturating_sub(Duration::from_nanos(now.timestamp_subsec_nanos().into()));
    glib::timeout_add_local_once(wait, move || tick(shared, generation));
}

/// Timers stop while the laptop sleeps, so the time is shown again as soon as it wakes.
async fn refresh_after_sleep(shared: Shared<String>) {
    let watch = async {
        let connection = zbus::Connection::system().await?;
        let mut signals = LoginProxy::new(&connection).await?.receive_prepare_for_sleep().await?;
        while let Some(signal) = signals.next().await {
            if !signal.args()?.start {
                show(&shared);
            }
        }
        zbus::Result::Ok(())
    };
    if let Err(err) = watch.await {
        eprintln!("clock: not watching for sleep: {err}");
    }
}
