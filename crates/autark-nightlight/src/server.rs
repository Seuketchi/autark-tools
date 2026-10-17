//! `org.autark.NightLight1` on the session bus, for Settings, the bar and Kith ("my screen is too bright").

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};

use crate::schedule::{Config, Kind};

pub const PATH: &str = "/org/autark/NightLight1";

pub struct Shared {
    pub config: Config,
    /// Turned on or off by hand, until the schedule next changes (or for good, with no schedule).
    pub manual: Option<(bool, Option<DateTime<Local>>)>,
    pub active: bool,
    pub temperature: u32,
}

impl Shared {
    /// On or off by hand, until the next sunset or sunrise (or the custom times') undoes it.
    pub fn set_by_hand(&mut self, on: bool) {
        let now = Local::now();
        let scheduled = self.config.depth(now) > 0.0;
        if on == scheduled {
            self.manual = None;
        } else {
            let until = self.config.nights(now).iter().flat_map(|(start, end)| [*start, *end]).filter(|time| *time > now).min();
            self.manual = Some((on, until));
        }
    }
}

pub struct NightLight {
    pub shared: Arc<Mutex<Shared>>,
    pub wake: async_channel::Sender<()>,
}

#[zbus::interface(name = "org.autark.NightLight1")]
impl NightLight {
    /// On or off now; the schedule takes over again at its next change.
    async fn set_active(&self, on: bool) {
        self.shared.lock().unwrap().set_by_hand(on);
        let _ = self.wake.try_send(());
    }

    async fn toggle(&self) {
        let mut shared = self.shared.lock().unwrap();
        let on = !shared.active;
        shared.set_by_hand(on);
        let _ = self.wake.try_send(());
    }

    /// Reads `nightlight.toml` again, after Settings changes it.
    async fn reload(&self) {
        let mut shared = self.shared.lock().unwrap();
        shared.config = Config::load();
        shared.manual = None;
        let _ = self.wake.try_send(());
    }

    #[zbus(property)]
    async fn active(&self) -> bool {
        self.shared.lock().unwrap().active
    }

    /// Kelvin now: 6500 in daylight, the configured temperature at night, in between while fading.
    #[zbus(property)]
    async fn temperature(&self) -> u32 {
        self.shared.lock().unwrap().temperature
    }

    /// "sun", "custom" (also "sun" without coordinates) or "off".
    #[zbus(property)]
    async fn schedule(&self) -> String {
        match self.shared.lock().unwrap().config.kind() {
            Kind::Sun => "sun",
            Kind::Custom => "custom",
            Kind::Off => "off",
        }
        .into()
    }

    /// Whether it was turned on or off by hand and the schedule hasn't taken over yet.
    #[zbus(property)]
    async fn manual(&self) -> bool {
        self.shared.lock().unwrap().manual.is_some()
    }
}
