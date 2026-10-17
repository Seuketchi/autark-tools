//! Kith's suggestions: code notices an event (see `events`), Ollama rates whether it's worth interrupting you
//! now, given Focus and what Kith remembers, and writes the card. A suggestion only offers: its one action runs
//! through `Act` once you click it. At most `per_hour` are shown in any hour, an event rated once isn't rated
//! again for a while, and "Don't suggest again" is kept, with the times shown, in
//! `$XDG_STATE_HOME/autark-ai/suggestions.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::events::{Builds, Event, Kind, Readings, Wifi};
use crate::server::Ai;

/// Where `autark-pkg` keeps its build logs unless `[suggestions]` says otherwise.
pub const BUILD_LOGS: &str = "/var/log/autark-pkg";

/// A rating at or above this shows the card.
pub const BAR: f64 = 0.5;

/// How long a suggestion stands; after that its card says it passed by.
pub const STANDS: u64 = 60 * 60;

/// `[suggestions]` in ai.toml.
#[derive(Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub per_hour: usize,
    /// The battery percentage, on battery, that's an event.
    pub low_battery: f64,
    /// `autark-pkg`'s `logs`, if pkg.toml moves them.
    pub build_logs: PathBuf,
}

impl Default for Limits {
    fn default() -> Self {
        Self { per_hour: 3, low_battery: 20.0, build_logs: PathBuf::from(BUILD_LOGS) }
    }
}

/// How long after rating an event the same kind and subject keeps quiet. A low battery comes once a discharge
/// and each build once, so they need none.
fn cooldown(kind: Kind) -> Duration {
    match kind {
        Kind::Runaway => Duration::from_secs(60 * 60),
        Kind::Wifi => Duration::from_secs(6 * 60 * 60),
        Kind::Battery | Kind::Build => Duration::ZERO,
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Never {
    pub key: String,
    pub label: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Kept {
    never: Vec<Never>,
    /// When each recent suggestion was shown, in seconds since the epoch.
    shown: Vec<u64>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("autark-ai/suggestions.json")
}

#[derive(Default)]
pub struct Policy {
    kept: Kept,
    quiet_until: HashMap<String, Instant>,
    /// Off in tests, so they don't write the state folder.
    saving: bool,
}

impl Policy {
    pub fn load() -> Self {
        let kept = std::fs::read(path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { kept, quiet_until: HashMap::new(), saving: true }
    }

    /// The first event worth rating: not one you said never to suggest, not rated lately, and only while
    /// fewer than `per_hour` were shown in the last hour. It then keeps quiet for its cooldown.
    pub fn pick(&mut self, events: Vec<Event>, now: u64, at: Instant, per_hour: usize) -> Option<Event> {
        self.quiet_until.retain(|_, until| *until > at);
        self.kept.shown.retain(|shown| now.saturating_sub(*shown) < 60 * 60);
        if self.kept.shown.len() >= per_hour {
            return None;
        }
        let event = events.into_iter().find(|event| {
            let key = event.key();
            !self.quiet_until.contains_key(&key) && !self.kept.never.iter().any(|never| never.key == key)
        })?;
        let cooldown = cooldown(event.kind);
        if !cooldown.is_zero() {
            self.quiet_until.insert(event.key(), at + cooldown);
        }
        Some(event)
    }

    pub fn shown(&mut self, now: u64) {
        self.kept.shown.push(now);
        self.save();
    }

    pub fn never(&mut self, key: &str, label: &str) {
        if !self.kept.never.iter().any(|never| never.key == key) {
            self.kept.never.push(Never { key: key.to_owned(), label: label.to_owned() });
            self.save();
        }
    }

    pub fn again(&mut self, key: &str) {
        self.kept.never.retain(|never| never.key != key);
        self.save();
    }

    pub fn nevers(&self) -> &[Never] {
        &self.kept.never
    }

    fn save(&self) {
        if !self.saving {
            return;
        }
        let path = path();
        let result = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let temporary = path.with_extension("json.saving");
            std::fs::write(&temporary, serde_json::to_vec(&self.kept)?)?;
            std::fs::rename(&temporary, &path)
        })();
        if let Err(err) = result {
            eprintln!("ai: saving {}: {err}", path.display());
        }
    }
}

#[zbus::proxy(interface = "org.autark.Stats1", default_service = "org.autark.Stats1", default_path = "/org/autark/Stats1")]
trait Stats {
    fn snapshot(&self) -> zbus::Result<String>;
}

#[zbus::proxy(interface = "org.autark.Notifications1", default_service = "org.freedesktop.Notifications", default_path = "/org/autark/Notifications")]
trait Centre {
    #[zbus(property)]
    fn focus(&self) -> zbus::Result<bool>;
}

/// How often `autark-stats` and iwd are looked at.
const LOOK_EVERY: Duration = Duration::from_secs(60);
/// How often a missing build log folder is looked for again.
const NO_LOGS_WAIT: Duration = Duration::from_secs(5 * 60);

/// Looks once a minute at `autark-stats` and iwd, and at the build logs whenever they change, and hands what
/// each look found to `Ai::consider`. Nothing is read while AI or suggestions are off.
pub async fn watch(connection: zbus::Connection) {
    let Ok(ai) = connection.object_server().interface::<_, Ai>(crate::server::PATH).await else { return };
    let looks = async {
        let (mut readings, mut wifi, mut system) = (Readings::default(), Wifi::default(), None);
        let (mut stats_said, mut iwd_said) = (false, false);
        loop {
            async_io::Timer::after(LOOK_EVERY).await;
            let Some(limits) = ai.get().await.suggesting() else { continue };
            let mut events = Vec::new();
            let snapshot = async { serde_json::from_str(&StatsProxy::new(&connection).await?.snapshot().await?).map_err(|err| zbus::Error::Failure(err.to_string())) };
            match snapshot.await {
                Ok(snapshot) => {
                    stats_said = false;
                    events.extend(readings.add(&snapshot, Instant::now(), limits.low_battery, std::process::id()));
                }
                Err(err) if !stats_said => {
                    stats_said = true;
                    eprintln!("ai: no readings from autark-stats for suggestions: {err}");
                }
                Err(_) => {}
            }
            if system.is_none() {
                system = zbus::Connection::system().await.ok();
            }
            if let Some(system) = &system {
                match wifi.look(system).await {
                    Ok(found) => {
                        iwd_said = false;
                        events.extend(found);
                    }
                    // A laptop without iwd has nothing to say about networks.
                    Err(err) if !iwd_said && !matches!(&err, zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown") => {
                        iwd_said = true;
                        eprintln!("ai: looking at Wi-Fi for suggestions: {err}");
                    }
                    Err(_) => {}
                }
            }
            if !events.is_empty() {
                ai.get().await.consider(ai.signal_emitter(), &connection, events).await;
            }
        }
    };
    let builds = async {
        loop {
            let folder = ai.get().await.limits().build_logs;
            match autark_common::changes::Changes::folder(&folder) {
                Ok(mut changes) => {
                    let mut builds = Builds::new(&folder);
                    while changes.next().await.is_ok() {
                        let events = builds.scan();
                        if !events.is_empty() && ai.get().await.suggesting().is_some() {
                            ai.get().await.consider(ai.signal_emitter(), &connection, events).await;
                        }
                    }
                }
                // No builds yet.
                Err(_) => {
                    async_io::Timer::after(NO_LOGS_WAIT).await;
                }
            }
        }
    };
    futures_util::future::join(looks, builds).await;
}

/// Whether Focus (do not disturb) is on; off if the centre doesn't say.
pub async fn focus(connection: &zbus::Connection) -> bool {
    let focus = async { CentreProxy::builder(connection).cache_properties(zbus::proxy::CacheProperties::No).build().await?.focus().await };
    focus.await.unwrap_or(false)
}

/// The suggestion as a quiet notification: kept in the centre, with no banner or chime.
pub async fn notify(connection: &zbus::Connection, text: &str) -> zbus::Result<u32> {
    let mut hints = HashMap::new();
    hints.insert("x-autark-quiet", zbus::Variant::from(true));
    hints.insert("desktop-entry", zbus::Variant::from("autark-ai"));
    crate::runner::NotificationsProxy::new(connection).await?.notify("Kith", 0, "", "Suggestion", text, &[], hints, -1).await
}

pub async fn close(connection: &zbus::Connection, id: u32) {
    let closed = async { crate::runner::NotificationsProxy::new(connection).await?.close_notification(id).await };
    if let Err(err) = closed.await {
        eprintln!("ai: closing a suggestion's notification: {err}");
    }
}

pub const SYSTEM: &str = "You are Kith, the assistant built into the Autark desktop. Something just happened on the user's laptop, \
                          noticed by code. Decide whether it's worth interrupting them with a suggestion card now.\n\
                          worth: from 0 to 1. Weigh how much it matters to them and whether the offer helps; with Focus on, only what \
                          can't wait is worth it. Use what you remember about them.\n\
                          text: one or two short, plain sentences for the card: what happened, then the offer as a question if there \
                          is one (\"Dim the screen to 40%?\"). Never say you've done anything.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": { "worth": { "type": "number" }, "text": { "type": "string" } },
        "required": ["worth", "text"],
    })
}

pub fn user(event: &Event, focus: bool, facts: &[String]) -> String {
    let offer = event.offer.as_ref().map_or("none".to_owned(), |(_, label)| label.clone());
    let mut user = format!("What happened: {}\nOffer: {offer}\nFocus (do not disturb): {}\nNow: {}", event.facts, if focus { "on" } else { "off" }, chrono::Local::now().format("%A %H:%M"));
    if !facts.is_empty() {
        let facts: Vec<String> = facts.iter().map(|fact| format!("- {fact}")).collect();
        user.push_str(&format!("\nWhat you remember about the user:\n{}", facts.join("\n")));
    }
    user
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::Action;

    fn event(kind: Kind, subject: &str) -> Event {
        Event { kind, subject: subject.into(), label: String::new(), facts: String::new(), offer: Some((Action::Mute { on: true }, "Mute".into())) }
    }

    #[test]
    fn three_an_hour_and_quiet_after_rating() {
        let mut policy = Policy::default();
        let (now, at) = (1_000_000, Instant::now());
        let picked = policy.pick(vec![event(Kind::Runaway, "kitchend"), event(Kind::Runaway, "editor")], now, at, 3).unwrap();
        assert_eq!(picked.subject, "kitchend");
        // kitchend keeps quiet for an hour, so editor comes next.
        assert_eq!(policy.pick(vec![event(Kind::Runaway, "kitchend"), event(Kind::Runaway, "editor")], now, at, 3).unwrap().subject, "editor");
        assert!(policy.pick(vec![event(Kind::Runaway, "kitchend")], now, at + Duration::from_secs(600), 3).is_none());
        assert!(policy.pick(vec![event(Kind::Runaway, "kitchend")], now, at + Duration::from_secs(3601), 3).is_some());
        for _ in 0..3 {
            policy.shown(now);
        }
        assert!(policy.pick(vec![event(Kind::Build, "")], now + 60, at, 3).is_none());
        assert!(policy.pick(vec![event(Kind::Build, "")], now + 3600, at, 3).is_some());
    }

    #[test]
    fn never_again_is_by_kind_and_subject() {
        let mut policy = Policy::default();
        let at = Instant::now();
        policy.never("wifi:CafeNet", "Joining “CafeNet”");
        assert!(policy.pick(vec![event(Kind::Wifi, "CafeNet")], 0, at, 3).is_none());
        assert_eq!(policy.pick(vec![event(Kind::Wifi, "CafeNet"), event(Kind::Wifi, "Library")], 0, at, 3).unwrap().subject, "Library");
        policy.again("wifi:CafeNet");
        assert!(policy.pick(vec![event(Kind::Wifi, "CafeNet")], 0, at, 3).is_some());
    }
}
