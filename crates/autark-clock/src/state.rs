//! What the clock remembers: the alarms, and the timer that's running, in `$XDG_STATE_HOME/autark/clock.toml`. The
//! timers that actually ring are systemd's; this is what the window shows and what it needs to take them back.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alarm {
    /// Made from the time it was set, so it's the name of its systemd units.
    pub id: u64,
    pub hour: u8,
    pub minute: u8,
    /// Which days it rings, as `Mon` to `Sun`; none means once, at the next `hour:minute`.
    pub days: Vec<String>,
    /// For a one-time alarm, the date it rings (`2026-10-20`).
    pub date: Option<String>,
    pub label: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timer {
    /// The systemd unit counting it down.
    pub unit: String,
    /// When it rings, in seconds since the epoch, and how long it was set for.
    pub end: u64,
    pub total: u64,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub alarms: Vec<Alarm>,
    pub timer: Option<Timer>,
}

fn path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("autark/clock.toml")
}

impl State {
    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(path()) else { return Self::default() };
        toml::from_str(&text).unwrap_or_else(|error| {
            eprintln!("clock: {}: {error}", path().display());
            Self::default()
        })
    }

    pub fn save(&self) {
        let path = path();
        let result = toml::to_string(self)
            .map_err(std::io::Error::other)
            .and_then(|text| path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| autark_common::files::write_atomically(&path, text.as_bytes())));
        if let Err(error) = result {
            eprintln!("clock: saving {}: {error}", path.display());
        }
    }
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_an_empty_file_is_a_clean_start() {
        let state = State {
            alarms: vec![Alarm { id: 7, hour: 6, minute: 45, days: vec!["Mon".into(), "Fri".into()], date: None, label: "Gym".into(), enabled: true }],
            timer: Some(Timer { unit: "autark-timer-1".into(), end: 1_800_000_000, total: 300 }),
        };
        assert_eq!(toml::from_str::<State>(&toml::to_string(&state).unwrap()).unwrap(), state);
        assert_eq!(toml::from_str::<State>("").unwrap(), State::default());
    }
}
