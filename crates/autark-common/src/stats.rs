//! What `autark-stats` publishes on `org.autark.Stats1`, as JSON. Rates are per second, sizes
//! in bytes, CPU in percent of one core (so a busy four-core machine reads up to 400 per app,
//! and 100 for the whole system).

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Snapshot {
    /// Unix time in milliseconds.
    pub time: u64,
    pub cpu: Cpu,
    pub memory: Memory,
    pub disk: Rate,
    pub network: Rate,
    /// What the machine draws right now, when it can be measured.
    pub power: Option<Power>,
    pub battery: Option<Battery>,
    /// Your apps, busiest first.
    pub apps: Vec<App>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Cpu {
    /// The whole system, 0 to 100.
    pub total: f64,
    /// Each core, 0 to 100.
    pub cores: Vec<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Rate {
    /// Disk reads, or network bytes received.
    pub read: u64,
    /// Disk writes, or network bytes sent.
    pub write: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Power {
    pub watts: f64,
    /// `rapl` (the CPU package's own counter) or `battery` (the whole laptop, on battery).
    pub source: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Battery {
    pub percent: f64,
    /// `Charging`, `Discharging`, `Full` or `Not charging`, as the kernel says it.
    pub status: String,
    /// Today's full charge against the design capacity, in percent.
    pub health: Option<f64>,
    pub cycles: Option<u64>,
    /// Draw while discharging, or charge rate while charging.
    pub watts: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct App {
    /// The app's cgroup, relative to `/sys/fs/cgroup`. Programs that share the login session's
    /// cgroup (started from a launcher or a key binding, not through systemd) get one entry each,
    /// told apart by `#` and the pid that started them: `/user.slice/user-1000.slice/session-1.scope#2042`.
    pub id: String,
    pub name: String,
    /// The desktop file id, when the app has one.
    pub desktop_id: Option<String>,
    /// Part of the desktop itself (the compositor, bar, dock, lock screen), which force quit doesn't offer.
    #[serde(default)]
    pub system: bool,
    pub cpu: f64,
    pub memory: u64,
    pub disk: Rate,
    /// The app's share of `power`, by CPU time.
    pub watts: Option<f64>,
    pub processes: Vec<Process>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub cpu: f64,
    pub memory: u64,
}
