//! `org.autark.Stats1` on the session bus. Clients call `Watch` and get `Updated` signals for as
//! long as they're connected; with nobody watching, nothing is read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::message::Header;
use zbus::object_server::SignalEmitter;

use crate::sample::Sampler;

pub const PATH: &str = "/org/autark/Stats1";

/// Watchers by unique bus name, with the interval each asked for in milliseconds.
pub type Watchers = Arc<Mutex<HashMap<String, u32>>>;

pub struct Stats {
    pub watchers: Watchers,
    pub sampler: Arc<Mutex<Sampler>>,
    pub latest: Arc<Mutex<String>>,
    /// Wakes the polling loop when watchers change.
    pub wake: async_channel::Sender<()>,
}

#[zbus::interface(name = "org.autark.Stats1")]
impl Stats {
    /// Starts `Updated` signals every `interval_ms` (500 to 60000) until `Unwatch` or until the
    /// caller leaves the bus. With several watchers the shortest interval wins.
    async fn watch(&self, #[zbus(header)] header: Header<'_>, interval_ms: u32) -> zbus::fdo::Result<()> {
        let sender = header.sender().ok_or_else(|| zbus::fdo::Error::Failed("no sender".to_owned()))?;
        self.watchers.lock().unwrap().insert(sender.to_string(), interval_ms.clamp(500, 60_000));
        let _ = self.wake.try_send(());
        Ok(())
    }

    async fn unwatch(&self, #[zbus(header)] header: Header<'_>) {
        if let Some(sender) = header.sender() {
            self.watchers.lock().unwrap().remove(sender.as_str());
            let _ = self.wake.try_send(());
        }
    }

    /// One reading now, as JSON (see `autark_common::stats::Snapshot`). Rates need two
    /// readings, so without a recent one this takes a third of a second.
    async fn snapshot(&self) -> String {
        let fresh = self.sampler.lock().unwrap().age().is_some_and(|age| age < Duration::from_secs(2));
        if fresh {
            return self.latest.lock().unwrap().clone();
        }
        let sampler = self.sampler.clone();
        blocking::unblock(move || sampler.lock().unwrap().sample()).await;
        async_io::Timer::after(Duration::from_millis(330)).await;
        let sampler = self.sampler.clone();
        let snapshot = blocking::unblock(move || sampler.lock().unwrap().sample()).await;
        let json = serde_json::to_string(&snapshot).unwrap_or_default();
        *self.latest.lock().unwrap() = json.clone();
        json
    }

    #[zbus(signal)]
    pub async fn updated(emitter: &SignalEmitter<'_>, snapshot: &str) -> zbus::Result<()>;
}
