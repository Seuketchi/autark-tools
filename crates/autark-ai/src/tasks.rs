//! Kith's tasks: a sentence to Kith becomes a task you confirm on its card, then a systemd user timer,
//! `autark-task@<id>.timer`, whose service runs `autark-ai task <id>` (see `runner`). Code owns the schedule:
//! the model picks from a few shapes and code writes the calendar. Each task keeps its folder,
//! `~/.local/share/autark-ai/tasks/<id>/`, with `task.toml` and its results.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use zbus::OwnedObjectPath;

#[zbus::proxy(interface = "org.freedesktop.systemd1.Manager", default_service = "org.freedesktop.systemd1", default_path = "/org/freedesktop/systemd1")]
trait Manager {
    fn load_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn start_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn enable_unit_files(&self, files: &[&str], runtime: bool, force: bool) -> zbus::Result<(bool, Vec<(String, String, String)>)>;
    fn reload(&self) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.systemd1.Timer", default_service = "org.freedesktop.systemd1")]
trait Timer {
    #[zbus(property, name = "NextElapseUSecRealtime")]
    fn next_elapse(&self) -> zbus::Result<u64>;
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Task {
    pub name: String,
    /// What to do on each run, said as an instruction to Kith.
    pub prompt: String,
    pub when: When,
    /// Files it reads, as `~/…`, under Ask Kith's rules.
    #[serde(default)]
    pub reads: Vec<String>,
    /// Pages it fetches, https only.
    #[serde(default)]
    pub urls: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct When {
    /// `day`, `weekdays`, `weekends`, a weekday (`monday`…) or `month`.
    pub every: String,
    /// The day of the month, for `month`.
    #[serde(default)]
    pub day: u8,
    /// `HH:MM`.
    pub time: String,
}

const WEEKDAYS: [(&str, &str); 7] = [("monday", "Mon"), ("tuesday", "Tue"), ("wednesday", "Wed"), ("thursday", "Thu"), ("friday", "Fri"), ("saturday", "Sat"), ("sunday", "Sun")];

impl When {
    /// The `OnCalendar` spec, or why the schedule isn't one of the shapes.
    pub fn calendar(&self) -> Result<String, String> {
        let time = self.time.trim();
        let (hour, minute) = time
            .split_once(':')
            .and_then(|(h, m)| Some((h.parse::<u8>().ok()?, m.parse::<u8>().ok()?)))
            .filter(|(h, m)| *h < 24 && *m < 60)
            .ok_or_else(|| format!("“{time}” isn't a time like 08:00"))?;
        let at = format!("{hour:02}:{minute:02}:00");
        let every = self.every.trim().to_lowercase();
        Ok(match every.as_str() {
            "day" => format!("*-*-* {at}"),
            "weekdays" => format!("Mon..Fri *-*-* {at}"),
            "weekends" => format!("Sat,Sun *-*-* {at}"),
            "month" if (1..=31).contains(&self.day) => format!("*-*-{:02} {at}", self.day),
            "month" => return Err(format!("{} isn't a day of the month", self.day)),
            day => match WEEKDAYS.iter().find(|(name, _)| *name == day) {
                Some((_, short)) => format!("{short} *-*-* {at}"),
                None => return Err(format!("“{every}” isn't a schedule Kith knows")),
            },
        })
    }
}

/// How long one run may take, and how many tasks there may be: `[tasks]` in ai.toml.
#[derive(Deserialize, Clone, Copy)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max: usize,
    pub minutes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max: 20, minutes: 5 }
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn folder() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("autark-ai/tasks")
}

fn units() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("systemd/user")
}

pub fn load(id: &str) -> Result<Task, String> {
    // Ids are made by `id_for`, so anything else (a path, say) isn't one.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!("“{id}” isn't a task id"));
    }
    let path = folder().join(id).join("task.toml");
    let text = std::fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    toml::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))
}

/// Checks a task as it would be created: a name, an instruction, a schedule of a known shape, and inputs Kith
/// may read. Returns it tidied.
pub fn check(mut task: Task) -> Result<Task, String> {
    task.name = task.name.trim().chars().take(60).collect();
    task.prompt = task.prompt.trim().to_owned();
    if task.name.is_empty() {
        return Err("the task needs a name".to_owned());
    }
    if task.prompt.is_empty() {
        return Err("the task needs something to do".to_owned());
    }
    if task.name.contains(['\n', '%']) {
        return Err("a task's name can't hold a new line or %".to_owned());
    }
    task.when.calendar()?;
    let mut reads = Vec::new();
    for path in &task.reads {
        let resolved = crate::files::allowed(path.trim()).map_err(|why| format!("{path}: {why}"))?;
        let shown = crate::files::shown(&resolved);
        if !reads.contains(&shown) {
            reads.push(shown);
        }
    }
    task.reads = reads;
    for url in &task.urls {
        crate::web::allowed(url)?;
    }
    task.urls.dedup();
    Ok(task)
}

/// A task's id from its name: "Morning build summary" is `morning-build-summary`, numbered when taken.
fn id_for(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    let slug = if slug.is_empty() { "task".to_owned() } else { slug };
    (1..).map(|n| if n == 1 { slug.clone() } else { format!("{slug}-{n}") }).find(|id| !taken(id)).expect("an endless range")
}

pub struct Created {
    pub id: String,
    pub unit: String,
    /// Microseconds since the Unix epoch, 0 if systemd didn't say.
    pub next: u64,
}

/// Writes the task, its timer and the shared service, then enables and starts the timer.
pub async fn create(connection: &zbus::Connection, task: Task, limits: Limits, runner: &str) -> Result<Created, String> {
    let task = check(task)?;
    let calendar = task.when.calendar()?;
    let tasks = folder();
    let count = std::fs::read_dir(&tasks).map(|entries| entries.flatten().filter(|entry| entry.path().is_dir()).count()).unwrap_or(0);
    if count >= limits.max {
        return Err(format!("there are {count} tasks already, the most allowed (`max` under [tasks] in ai.toml)"));
    }
    let units = units();
    let id = id_for(&task.name, |id| tasks.join(id).exists() || units.join(format!("autark-task@{id}.timer")).exists());
    let unit = format!("autark-task@{id}.timer");
    let failed = |path: &std::path::Path, err: std::io::Error| format!("{}: {err}", path.display());

    let dir = tasks.join(&id);
    std::fs::create_dir_all(&dir).map_err(|err| failed(&dir, err))?;
    let spec = toml::to_string(&task).map_err(|err| err.to_string())?;
    std::fs::write(dir.join("task.toml"), spec).map_err(|err| failed(&dir, err))?;

    std::fs::create_dir_all(&units).map_err(|err| failed(&units, err))?;
    let service =
        format!("# Written by autark-ai: runs one of Kith's tasks.\n[Unit]\nDescription=Kith task %i\n\n[Service]\nType=oneshot\nExecStart={runner} task %i\nTimeoutStartSec={}min\n", limits.minutes);
    let service_path = units.join("autark-task@.service");
    std::fs::write(&service_path, service).map_err(|err| failed(&service_path, err))?;
    let timer = format!("# Written by autark-ai for Kith's task {id}.\n[Unit]\nDescription={}\n\n[Timer]\nOnCalendar={calendar}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n", task.name);
    let timer_path = units.join(&unit);
    std::fs::write(&timer_path, timer).map_err(|err| failed(&timer_path, err))?;

    let started = async {
        let manager = ManagerProxy::new(connection).await?;
        manager.reload().await?;
        manager.enable_unit_files(&[&unit], false, false).await?;
        manager.start_unit(&unit, "replace").await
    };
    if let Err(err) = started.await {
        // Nothing half made is left behind, so trying again doesn't make a second task.
        let _ = std::fs::remove_file(&timer_path);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("systemd’s user manager: {err}"));
    }
    // The first run is only for the card to say; the task is made either way.
    let next = async {
        let path = ManagerProxy::new(connection).await?.load_unit(&unit).await?;
        TimerProxy::builder(connection).path(path).build().await?.next_elapse().await
    };
    let next = next.await.unwrap_or_else(|err| {
        eprintln!("ai: reading when {unit} first runs: {err}");
        0
    });
    Ok(Created { id, unit, next })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn when(every: &str, day: u8, time: &str) -> When {
        When { every: every.into(), day, time: time.into() }
    }

    #[test]
    fn schedules_become_calendars() {
        assert_eq!(when("day", 0, "8:00").calendar().unwrap(), "*-*-* 08:00:00");
        assert_eq!(when("weekdays", 0, "07:30").calendar().unwrap(), "Mon..Fri *-*-* 07:30:00");
        assert_eq!(when("Monday", 0, "09:00").calendar().unwrap(), "Mon *-*-* 09:00:00");
        assert_eq!(when("month", 1, "00:00").calendar().unwrap(), "*-*-01 00:00:00");
        assert!(when("month", 0, "08:00").calendar().is_err());
        assert!(when("hourly", 0, "08:00").calendar().is_err());
        assert!(when("day", 0, "25:00").calendar().is_err());
    }

    #[test]
    fn ids_come_from_names() {
        assert_eq!(id_for("Morning build summary", |_| false), "morning-build-summary");
        assert_eq!(id_for("BLFS: advisories!", |id| id == "blfs-advisories"), "blfs-advisories-2");
        assert_eq!(id_for("✦✦", |_| false), "task");
        assert!(load("../../.ssh").unwrap_err().contains("isn't a task id"));
    }

    #[test]
    fn tasks_are_checked() {
        let task = Task { name: " Peek ".into(), prompt: "Summarise it".into(), when: when("day", 0, "08:00"), reads: vec!["~/.ssh/config".into()], urls: Vec::new() };
        assert!(check(task.clone()).is_err());
        let task = Task { reads: vec!["~/autark/log.md".into(), "autark/log.md".into()], urls: vec!["http://example.com".into()], ..task };
        assert!(check(task.clone()).is_err());
        let task = check(Task { urls: vec!["https://www.linuxfromscratch.org/blfs/advisories/".into()], ..task }).unwrap();
        assert_eq!(task.name, "Peek");
        assert_eq!(task.reads, vec!["~/autark/log.md"]);
    }
}
