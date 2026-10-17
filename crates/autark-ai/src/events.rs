//! What Kith's suggestions start from, all noticed by code: an app keeping a core busy or draining the
//! battery for minutes, and the battery running low, from one `autark-stats` reading a minute (each an
//! average over that minute, so it costs almost nothing); an `autark-pkg` build ending, from the outcome
//! line it writes at the end of each log; and a Wi-Fi network you haven't joined in range while the laptop
//! is offline, from iwd.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use autark_common::stats::{App, Snapshot};
use serde::{Deserialize, Serialize};
use zbus::OwnedVariant;

use crate::route::Action;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Runaway,
    Battery,
    Build,
    Wifi,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: Kind,
    /// With the kind, what "Don't suggest again" covers: the app or the network, or empty for the whole kind.
    pub subject: String,
    /// How the list of what's never suggested names it: "Quitting Firefox".
    pub label: String,
    /// What happened, in words, for the model to rate.
    pub facts: String,
    /// The one thing the card offers, with its button's label.
    pub offer: Option<(Action, String)>,
}

impl Event {
    pub fn key(&self) -> String {
        format!("{}:{}", serde_json::to_value(self.kind).ok().and_then(|kind| kind.as_str().map(str::to_owned)).unwrap_or_default(), self.subject)
    }
}

/// An app above this, in percent of a core, counts as busy.
const BUSY: f64 = 80.0;
/// On battery, an app drawing this many watts counts as busy too.
const DRAIN_WATTS: f64 = 5.0;
/// How long an app stays busy before it's an event.
const BUSY_FOR: Duration = Duration::from_secs(120);

/// What the minute's readings have shown so far.
#[derive(Default)]
pub struct Readings {
    busy_since: HashMap<String, Instant>,
    /// Said once each time the battery runs down, until it's plugged in.
    battery_said: bool,
}

impl Readings {
    /// Takes one reading and returns its events: the apps busy long enough, busiest first, and a low battery.
    /// `me` is `autark-ai`'s own process, never suggested.
    pub fn add(&mut self, snapshot: &Snapshot, at: Instant, low_battery: f64, me: u32) -> Vec<Event> {
        let draining = snapshot.battery.as_ref().is_some_and(|battery| battery.status == "Discharging");
        let busy = |app: &App| app.cpu >= BUSY || (draining && app.watts.is_some_and(|watts| watts >= DRAIN_WATTS));
        self.busy_since.retain(|id, _| snapshot.apps.iter().any(|app| &app.id == id && busy(app)));
        for app in snapshot.apps.iter().filter(|app| busy(app)) {
            self.busy_since.entry(app.id.clone()).or_insert(at);
        }
        let mut apps: Vec<&App> = snapshot
            .apps
            .iter()
            .filter(|app| self.busy_since.get(&app.id).is_some_and(|since| at.duration_since(*since) >= BUSY_FOR))
            // Only apps you started, and never this one.
            .filter(|app| app.id.contains("/app.slice/") && !app.processes.iter().any(|process| process.pid == me))
            .collect();
        apps.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        let mut events: Vec<Event> = apps
            .into_iter()
            .map(|app| {
                let minutes = at.duration_since(self.busy_since[&app.id]).as_secs() / 60;
                let what = if app.cpu >= BUSY {
                    format!("{} has kept a core busy ({:.0}% of one) for {minutes} minutes", app.name, app.cpu)
                } else {
                    format!("{} has been drawing {:.0} W for {minutes} minutes", app.name, app.watts.unwrap_or_default())
                };
                let battery = match &snapshot.battery {
                    Some(battery) if draining => format!(", with the laptop on battery at {:.0}%", battery.percent),
                    _ => String::new(),
                };
                Event {
                    kind: Kind::Runaway,
                    subject: app.desktop_id.clone().unwrap_or_else(|| app.name.clone()),
                    label: format!("Quitting {}", app.name),
                    facts: format!("{what}{battery}."),
                    offer: Some((Action::QuitApp { app: app.id.clone() }, format!("Quit {}", app.name))),
                }
            })
            .collect();

        match &snapshot.battery {
            Some(battery) if draining => {
                if battery.percent <= low_battery && battery.percent > 0.0 && !self.battery_said {
                    self.battery_said = true;
                    events.push(Event {
                        kind: Kind::Battery,
                        subject: String::new(),
                        label: "Low battery".to_owned(),
                        facts: format!("The battery is at {:.0}% and running down; it isn't plugged in.", battery.percent),
                        offer: Some((Action::SetBrightness { percent: 40 }, "Dim to 40%".to_owned())),
                    });
                }
            }
            _ => self.battery_said = false,
        }
        events
    }
}

/// What `autark-pkg` writes as the last line of a build's log: `== autark-pkg: gcc 15.2.0 built in 812s`, or
/// `== autark-pkg: gcc 15.2.0 failed (exit status: 2)`.
pub const OUTCOME: &str = "== autark-pkg: ";

/// The build logs, each looked at again when it changes.
pub struct Builds {
    folder: PathBuf,
    seen: HashMap<PathBuf, SystemTime>,
}

impl Builds {
    /// Starts from the logs there now, so only builds that end from here on are events.
    pub fn new(folder: &Path) -> Self {
        let mut builds = Self { folder: folder.to_owned(), seen: HashMap::new() };
        builds.scan_quietly();
        builds
    }

    /// The builds that have ended since the last look.
    pub fn scan(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.folder) else { return events };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else { continue };
            if path.extension().is_none_or(|extension| extension != "log") || self.seen.get(&path) == Some(&modified) {
                continue;
            }
            self.seen.insert(path.clone(), modified);
            events.extend(build_ended(&path));
        }
        events
    }

    /// Records every log as seen, for the first look.
    fn scan_quietly(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.folder) else { return };
        for entry in entries.flatten() {
            if let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) {
                self.seen.insert(entry.path(), modified);
            }
        }
    }
}

/// The build a log tells of, if its last line says how it ended; a failed one comes with the lines before.
fn build_ended(path: &Path) -> Option<Event> {
    let tail = tail(path, 4096)?;
    let lines: Vec<&str> = tail.lines().filter(|line| !line.trim().is_empty()).collect();
    let (last, before) = lines.split_last()?;
    let outcome = last.strip_prefix(OUTCOME)?.trim();
    let mut facts = format!("An autark-pkg build ended: {outcome}.");
    if outcome.contains(" failed") {
        let shown: Vec<&str> = before.iter().rev().take(12).rev().copied().collect();
        facts.push_str(&format!(" The log's last lines:\n{}", shown.join("\n")));
    }
    Some(Event {
        kind: Kind::Build,
        subject: String::new(),
        label: "Builds ending".to_owned(),
        facts,
        offer: Some((Action::OpenFile { path: path.to_string_lossy().into_owned() }, "Open the log".to_owned())),
    })
}

/// The last `bytes` of a file, from a line's start.
fn tail(path: &Path, bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(bytes))).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer).into_owned();
    Some(if length > bytes { text.split_once('\n').map_or(text.clone(), |(_, rest)| rest.to_owned()) } else { text })
}

type Properties = HashMap<String, OwnedVariant>;

fn interface<'a>(interfaces: &'a BTreeMap<zbus::names::OwnedInterfaceName, Properties>, name: &str) -> Option<&'a Properties> {
    interfaces.iter().find(|(key, _)| key.as_str() == name).map(|(_, properties)| properties)
}

fn text(properties: &Properties, key: &str) -> String {
    properties.get(key).and_then(|value| String::try_from(value.try_clone().ok()?).ok()).unwrap_or_default()
}

/// Whether the laptop has been offline for two looks running, and the Wi-Fi networks in range it hasn't joined.
#[derive(Default)]
pub struct Wifi {
    offline_looks: u32,
}

impl Wifi {
    /// One look at iwd: the networks you haven't joined, open ones first, once the laptop has been offline a
    /// while. None while it's connected, while Wi-Fi is off, or on a laptop without iwd.
    pub async fn look(&mut self, system: &zbus::Connection) -> zbus::Result<Vec<Event>> {
        let objects = zbus::fdo::ObjectManagerProxy::builder(system).destination("net.connman.iwd").path("/").build().await?.get_managed_objects().await?;
        let states: Vec<String> = objects.values().filter_map(|interfaces| interface(interfaces, "net.connman.iwd.Station")).map(|station| text(station, "State")).collect();
        if states.is_empty() || states.iter().any(|state| state != "disconnected") {
            self.offline_looks = 0;
            return Ok(Vec::new());
        }
        self.offline_looks += 1;
        if self.offline_looks < 2 {
            return Ok(Vec::new());
        }
        let mut networks: Vec<(String, String)> = objects
            .values()
            .filter_map(|interfaces| interface(interfaces, "net.connman.iwd.Network"))
            .filter(|network| !network.contains_key("KnownNetwork"))
            .map(|network| (text(network, "Name"), text(network, "Type")))
            .filter(|(name, _)| !name.is_empty())
            .collect();
        networks.sort_by_key(|(_, kind)| kind != "open");
        networks.truncate(3);
        Ok(networks
            .into_iter()
            .map(|(name, kind)| {
                let security = match kind.as_str() {
                    "open" => "open, no password",
                    "8021x" => "an enterprise network, which needs a provisioning file",
                    _ => "secured with a password",
                };
                Event {
                    kind: Kind::Wifi,
                    subject: name.clone(),
                    label: format!("Joining “{name}”"),
                    facts: format!("The laptop has been offline for a few minutes. A Wi-Fi network it has never joined is in range: “{name}” ({security})."),
                    offer: Some((Action::OpenSettings { pane: "wifi".to_owned() }, "Open Wi-Fi settings".to_owned())),
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autark_common::stats::{Battery, Process};

    fn app(name: &str, cpu: f64, watts: Option<f64>) -> App {
        App {
            id: format!("/user.slice/user-1000.slice/user@1000.service/app.slice/app-{name}.scope"),
            name: name.to_owned(),
            cpu,
            watts,
            processes: vec![Process { pid: 4242, name: name.to_owned(), ..Default::default() }],
            ..Default::default()
        }
    }

    fn snapshot(apps: Vec<App>, battery: Option<(&str, f64)>) -> Snapshot {
        let battery = battery.map(|(status, percent)| Battery { status: status.to_owned(), percent, ..Default::default() });
        Snapshot { apps, battery, ..Default::default() }
    }

    #[test]
    fn an_app_busy_for_two_minutes() {
        let mut readings = Readings::default();
        let start = Instant::now();
        let busy = snapshot(vec![app("kitchend", 150.0, None), app("editor", 3.0, None)], None);
        assert!(readings.add(&busy, start, 20.0, 1).is_empty());
        assert!(readings.add(&busy, start + Duration::from_secs(60), 20.0, 1).is_empty());
        let events = readings.add(&busy, start + Duration::from_secs(120), 20.0, 1);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].facts, "kitchend has kept a core busy (150% of one) for 2 minutes.");
        assert_eq!(events[0].key(), "runaway:kitchend");
        assert_eq!(events[0].offer.as_ref().unwrap().1, "Quit kitchend");
        // A quiet minute starts it over.
        readings.add(&snapshot(vec![app("kitchend", 10.0, None)], None), start + Duration::from_secs(180), 20.0, 1);
        assert!(readings.add(&busy, start + Duration::from_secs(240), 20.0, 1).is_empty());
    }

    #[test]
    fn drain_counts_only_on_battery() {
        let mut readings = Readings::default();
        let start = Instant::now();
        let hungry = |status| snapshot(vec![app("kitchend", 20.0, Some(9.0))], Some((status, 80.0)));
        readings.add(&hungry("Charging"), start, 20.0, 1);
        assert!(readings.add(&hungry("Charging"), start + Duration::from_secs(180), 20.0, 1).is_empty());
        readings.add(&hungry("Discharging"), start + Duration::from_secs(240), 20.0, 1);
        let events = readings.add(&hungry("Discharging"), start + Duration::from_secs(360), 20.0, 1);
        assert_eq!(events[0].facts, "kitchend has been drawing 9 W for 2 minutes, with the laptop on battery at 80%.");
    }

    #[test]
    fn a_low_battery_once_a_discharge() {
        let mut readings = Readings::default();
        let at = Instant::now();
        assert!(readings.add(&snapshot(Vec::new(), Some(("Discharging", 25.0))), at, 20.0, 1).is_empty());
        assert_eq!(readings.add(&snapshot(Vec::new(), Some(("Discharging", 19.0))), at, 20.0, 1)[0].key(), "battery:");
        assert!(readings.add(&snapshot(Vec::new(), Some(("Discharging", 18.0))), at, 20.0, 1).is_empty());
        readings.add(&snapshot(Vec::new(), Some(("Charging", 18.0))), at, 20.0, 1);
        assert_eq!(readings.add(&snapshot(Vec::new(), Some(("Discharging", 17.0))), at, 20.0, 1).len(), 1);
    }

    #[test]
    fn a_build_ending_is_read_from_its_log() {
        let folder = std::env::temp_dir().join(format!("autark-ai-builds-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("old.log"), "make\n== autark-pkg: old 1.0 built in 3s\n").unwrap();
        let mut builds = Builds::new(&folder);
        assert!(builds.scan().is_empty());
        std::fs::write(folder.join("gcc.log"), "make: *** [all] Error 2\n").unwrap();
        assert!(builds.scan().is_empty());
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(folder.join("gcc.log"), "checking\nmake: *** [all] Error 2\n== autark-pkg: gcc 15.2.0 failed (exit status: 2)\n").unwrap();
        let events = builds.scan();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].facts, "An autark-pkg build ended: gcc 15.2.0 failed (exit status: 2). The log's last lines:\nchecking\nmake: *** [all] Error 2");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
