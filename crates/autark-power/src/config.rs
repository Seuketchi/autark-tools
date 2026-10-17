//! `power.toml`: how long until the screens go off and the laptop sleeps, on battery and plugged in, and when
//! to warn about a low battery.
//!
//! ```toml
//! low_battery = 10    # percent; 0 never warns
//!
//! [battery]
//! screen_off = 2      # minutes idle; 0 is never
//! sleep = 10
//!
//! [plugged_in]
//! screen_off = 5
//! sleep = 30
//! ```

use std::time::Duration;

use serde::Deserialize;

/// Minutes; a key left out takes the default for its section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Timers {
    pub screen_off: Option<u32>,
    pub sleep: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Power {
    pub low_battery: u32,
    pub battery: Timers,
    pub plugged_in: Timers,
}

impl Default for Power {
    fn default() -> Self {
        Self { low_battery: 10, battery: Timers::default(), plugged_in: Timers::default() }
    }
}

impl Power {
    pub fn load() -> Self {
        autark_common::config::load("power")
    }

    /// The screen-off and sleep timeouts for now, none where it's never.
    pub fn timeouts(&self, on_battery: bool) -> (Option<Duration>, Option<Duration>) {
        let (timers, (screen_off, sleep)) = if on_battery { (self.battery, (2, 10)) } else { (self.plugged_in, (5, 30)) };
        let minutes = |minutes: u32| (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60));
        (minutes(timers.screen_off.unwrap_or(screen_off)), minutes(timers.sleep.unwrap_or(sleep)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_follow_the_power_source() {
        let power: Power = toml::from_str("[plugged_in]\nsleep = 0").unwrap();
        assert_eq!(power.timeouts(true), (Some(Duration::from_secs(120)), Some(Duration::from_secs(600))));
        assert_eq!(power.timeouts(false), (Some(Duration::from_secs(300)), None));
    }
}
