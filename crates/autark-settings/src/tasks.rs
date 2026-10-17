//! Kith's tasks in standalone mode: each is a systemd user timer, `autark-task@<id>.timer`, which starts
//! `autark-task@<id>.service`, with its results in `~/.local/share/autark-ai/tasks/<id>/`. A task shows here
//! while its timer is running or enabled, or while its folder is there, so a paused one stays listed.

use std::path::PathBuf;

use zbus::OwnedObjectPath;

#[zbus::proxy(interface = "org.freedesktop.systemd1.Manager", default_service = "org.freedesktop.systemd1", default_path = "/org/freedesktop/systemd1")]
trait Manager {
    fn list_units_by_patterns(&self, states: &[&str], patterns: &[&str]) -> zbus::Result<Vec<Unit>>;
    fn load_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn start_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn stop_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn enable_unit_files(&self, files: &[&str], runtime: bool, force: bool) -> zbus::Result<(bool, Vec<(String, String, String)>)>;
    fn disable_unit_files(&self, files: &[&str], runtime: bool) -> zbus::Result<Vec<(String, String, String)>>;
    fn reload(&self) -> zbus::Result<()>;
}

/// systemd's `ListUnits` row: name, description, load, active and sub state, followed unit, object path,
/// job ID, job type and job path.
type Unit = (String, String, String, String, String, String, OwnedObjectPath, u32, String, OwnedObjectPath);

#[zbus::proxy(interface = "org.freedesktop.systemd1.Unit", default_service = "org.freedesktop.systemd1")]
trait UnitProperties {
    #[zbus(property)]
    fn description(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn active_state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn unit_file_state(&self) -> zbus::Result<String>;
}

#[zbus::proxy(interface = "org.freedesktop.systemd1.Timer", default_service = "org.freedesktop.systemd1")]
trait Timer {
    #[zbus(property, name = "NextElapseUSecRealtime")]
    fn next_elapse(&self) -> zbus::Result<u64>;
    #[zbus(property)]
    fn timers_calendar(&self) -> zbus::Result<Vec<(String, String, u64)>>;
}

pub const DAEMON: &str = "systemd’s user manager";

#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    /// The timer's unit name, as systemd escapes it.
    pub unit: String,
    pub name: String,
    /// systemd's calendar spec, such as `*-*-* 08:00:00`.
    pub calendar: Option<String>,
    /// Microseconds since the Unix epoch; 0 when it won't run.
    pub next: u64,
    /// The timer is running and enabled.
    pub on: bool,
    /// Its service is running now.
    pub running: bool,
}

fn folder() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("autark-ai/tasks")
}

fn id_of(unit: &str) -> String {
    unit.trim_start_matches("autark-task@").trim_end_matches(".timer").replace("\\x2d", "-")
}

pub async fn list(connection: &zbus::Connection) -> zbus::Result<Vec<Task>> {
    let manager = ManagerProxy::new(connection).await?;
    let mut units: Vec<String> = manager.list_units_by_patterns(&[], &["autark-task@*.timer"]).await?.into_iter().map(|unit| unit.0).collect();
    if let Ok(entries) = std::fs::read_dir(folder()) {
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && !units.iter().any(|unit| id_of(unit) == id) {
                units.push(format!("autark-task@{id}.timer"));
            }
        }
    }
    let mut tasks = Vec::new();
    for unit in units {
        let id = id_of(&unit);
        let path = manager.load_unit(&unit).await?;
        let properties = UnitPropertiesProxy::builder(connection).path(path.clone()).build().await?;
        let active = properties.active_state().await? == "active";
        let enabled = properties.unit_file_state().await?.starts_with("enabled");
        let has_folder = folder().join(&id).is_dir();
        if !active && !enabled && !has_folder {
            continue;
        }
        let timer = TimerProxy::builder(connection).path(path).build().await?;
        let calendar = timer.timers_calendar().await.unwrap_or_default().into_iter().map(|(_, spec, _)| spec).next();
        let service = manager.load_unit(&unit.replace(".timer", ".service")).await?;
        let state = UnitPropertiesProxy::builder(connection).path(service).build().await?.active_state().await?;
        tasks.push(Task {
            name: name(&unit, &properties.description().await.unwrap_or_default()),
            calendar,
            next: if active { timer.next_elapse().await.unwrap_or(0) } else { 0 },
            on: active && enabled,
            running: state == "active" || state == "activating",
            id,
            unit,
        });
    }
    tasks.sort_by_key(|task| task.name.to_lowercase());
    Ok(tasks)
}

/// Kith writes each timer's description as the task's name; failing that, the id reads well enough
/// ("learning-digest" becomes "Learning digest"), as in the centre's Kith widget.
fn name(unit: &str, description: &str) -> String {
    if !description.trim().is_empty() && !description.contains(unit) && !description.contains("autark-task@") {
        return description.trim().to_owned();
    }
    let words = id_of(unit).replace(['-', '_'], " ");
    let mut chars = words.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
}

pub async fn run(connection: &zbus::Connection, task: &Task) -> zbus::Result<()> {
    ManagerProxy::new(connection).await?.start_unit(&task.unit.replace(".timer", ".service"), "replace").await.map(drop)
}

/// Resumes or pauses a task: its timer started and enabled, or stopped and disabled.
pub async fn set_on(connection: &zbus::Connection, task: &Task, on: bool) -> zbus::Result<()> {
    let manager = ManagerProxy::new(connection).await?;
    if on {
        manager.enable_unit_files(&[&task.unit], false, false).await?;
        manager.reload().await?;
        manager.start_unit(&task.unit, "replace").await?;
    } else {
        manager.stop_unit(&task.unit, "replace").await?;
        manager.disable_unit_files(&[&task.unit], false).await?;
        manager.reload().await?;
    }
    Ok(())
}

/// Stops and disables the timer and removes the task's folder.
pub async fn delete(connection: &zbus::Connection, task: &Task) -> Result<(), String> {
    set_on(connection, task, false).await.map_err(|error| crate::bus::message(error, DAEMON))?;
    let path = folder().join(&task.id);
    match std::fs::remove_dir_all(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {error}", path.display())),
        _ => Ok(()),
    }
}

/// A calendar spec in words, for the common shapes Kith writes: "Every day · 08:00", "Mondays · 09:00",
/// "1st of each month". Anything else is shown as systemd wrote it.
pub fn schedule(calendar: &str) -> String {
    let parts: Vec<&str> = calendar.split_whitespace().collect();
    let (days, date, time) = match parts.as_slice() {
        [date, time] => (None, *date, *time),
        [days, date, time] => (Some(*days), *date, *time),
        _ => return calendar.to_owned(),
    };
    let time = match time.strip_suffix(":00") {
        Some(time) if time.len() == 5 => time,
        _ => return calendar.to_owned(),
    };
    let when = match (days, date.split('-').collect::<Vec<_>>().as_slice()) {
        (None, ["*", "*", "*"]) => "Every day".to_owned(),
        (Some("Mon..Fri"), ["*", "*", "*"]) => "Weekdays".to_owned(),
        (Some("Sat,Sun"), ["*", "*", "*"]) => "Weekends".to_owned(),
        (Some(days), ["*", "*", "*"]) if !days.contains(['.', ',']) => match weekday(days) {
            Some(day) => format!("{day}s"),
            None => return calendar.to_owned(),
        },
        (Some(days), ["*", "*", "*"]) if !days.contains('.') => {
            let names: Option<Vec<&str>> = days.split(',').map(|day| weekday(day).map(|_| day)).collect();
            match names {
                Some(names) => names.join(", "),
                None => return calendar.to_owned(),
            }
        }
        (None, ["*", "*", day]) => match day.parse::<u32>() {
            Ok(day) => format!("{} of each month", ordinal(day)),
            Err(_) => return calendar.to_owned(),
        },
        _ => return calendar.to_owned(),
    };
    // Midnight is what a spec without a time means, so it's left unsaid.
    if time == "00:00" && when.ends_with("month") { when } else { format!("{when} · {time}") }
}

fn weekday(short: &str) -> Option<&'static str> {
    let names = [("Mon", "Monday"), ("Tue", "Tuesday"), ("Wed", "Wednesday"), ("Thu", "Thursday"), ("Fri", "Friday"), ("Sat", "Saturday"), ("Sun", "Sunday")];
    names.iter().find(|(name, _)| *name == short).map(|(_, long)| *long)
}

fn ordinal(day: u32) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendars_read_in_words() {
        assert_eq!(schedule("*-*-* 08:00:00"), "Every day · 08:00");
        assert_eq!(schedule("Mon *-*-* 09:00:00"), "Mondays · 09:00");
        assert_eq!(schedule("Mon..Fri *-*-* 07:30:00"), "Weekdays · 07:30");
        assert_eq!(schedule("Mon,Thu *-*-* 18:30:00"), "Mon, Thu · 18:30");
        assert_eq!(schedule("*-*-01 00:00:00"), "1st of each month");
        assert_eq!(schedule("*-*-22 06:00:00"), "22nd of each month · 06:00");
        assert_eq!(schedule("*-*-* *:00/15:00"), "*-*-* *:00/15:00");
    }

    #[test]
    fn unit_names_read_as_ids_and_names() {
        assert_eq!(id_of("autark-task@learning\\x2ddigest.timer"), "learning-digest");
        assert_eq!(name("autark-task@morning-build.timer", "Morning build summary"), "Morning build summary");
        assert_eq!(name("autark-task@morning-build.timer", "autark-task@morning-build.timer"), "Morning build");
    }
}
