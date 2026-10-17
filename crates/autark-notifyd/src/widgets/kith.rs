//! Kith's widget: its next scheduled tasks and its suggestion to quit a runaway app.
//!
//! Tasks are the `autark-task@<id>.timer` user timers of standalone mode; tasks that run on
//! Kith's own box arrive through `autark-ai` once it serves them. Runaway apps come from one
//! reading of `autark-stats` a minute, each an average over that minute, so spotting them costs
//! almost nothing and still works with the centre closed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use autark_common::stats::{App, Snapshot};
use gtk::glib;
use gtk::prelude::*;

use crate::shared::Shared;

#[zbus::proxy(interface = "org.freedesktop.systemd1.Manager", default_service = "org.freedesktop.systemd1", default_path = "/org/freedesktop/systemd1")]
trait Manager {
    fn list_units_by_patterns(&self, states: &[&str], patterns: &[&str]) -> zbus::Result<Vec<Unit>>;
}

/// systemd's `ListUnits` row: name, description, load, active and sub state, followed unit,
/// object path, job ID, job type and job path.
type Unit = (String, String, String, String, String, String, zbus::OwnedObjectPath, u32, String, zbus::OwnedObjectPath);

#[zbus::proxy(interface = "org.freedesktop.systemd1.Timer", default_service = "org.freedesktop.systemd1")]
trait Timer {
    #[zbus(property, name = "NextElapseUSecRealtime")]
    fn next_elapse(&self) -> zbus::Result<u64>;
}

#[derive(Clone)]
pub struct Task {
    pub name: String,
    /// Microseconds since the Unix epoch.
    pub next: u64,
}

/// Up to three tasks, soonest first.
pub async fn tasks(connection: &zbus::Connection) -> zbus::Result<Vec<Task>> {
    let units = ManagerProxy::new(connection).await?.list_units_by_patterns(&[], &["autark-task@*.timer"]).await?;
    let mut tasks = Vec::new();
    for (unit, description, _, _, _, _, path, ..) in units {
        let next = TimerProxy::builder(connection).path(path).build().await?.next_elapse().await?;
        if next == 0 {
            continue;
        }
        tasks.push(Task { name: task_name(&unit, &description), next });
    }
    tasks.sort_by_key(|task| task.next);
    tasks.truncate(3);
    Ok(tasks)
}

/// Kith writes each timer's description as the task's name; failing that, the instance name
/// reads well enough ("learning-digest" becomes "Learning digest").
fn task_name(unit: &str, description: &str) -> String {
    let id = unit.trim_start_matches("autark-task@").trim_end_matches(".timer").replace("\\x2d", "-");
    if !description.trim().is_empty() && !description.contains(unit) && !description.contains("autark-task@") {
        return description.trim().to_owned();
    }
    let words = id.replace(['-', '_'], " ");
    let mut chars = words.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
}

/// "18:30" today, "Sun" this week, "24 Oct" after.
fn when(next: u64, now: &glib::DateTime) -> String {
    let Ok(time) = glib::DateTime::from_unix_local((next / 1_000_000) as i64) else {
        return String::new();
    };
    let days = time.difference(now).as_seconds() / 86_400;
    let format = if time.day_of_year() == now.day_of_year() && time.year() == now.year() {
        "%H:%M"
    } else if days < 6 {
        "%a"
    } else {
        "%-d %b"
    };
    time.format(format).map(String::from).unwrap_or_default()
}

/// An app above this, in percent of a core, counts as busy.
const BUSY: f64 = 80.0;
/// On battery, an app drawing this many watts counts as busy too.
const DRAIN_WATTS: f64 = 5.0;
/// How long an app stays busy before Kith says so.
const SUGGEST_AFTER: Duration = Duration::from_secs(120);
/// How long "Not now" keeps quiet about an app.
const SNOOZE: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, PartialEq, Debug)]
pub struct Suggestion {
    pub app: String,
    pub name: String,
    /// What "Don't suggest again" remembers: the desktop ID, or the name.
    pub key: String,
    pub text: String,
    pub pids: Vec<u32>,
}

#[derive(Default)]
pub struct Runaways {
    busy_since: HashMap<String, Instant>,
    snoozed: HashMap<String, Instant>,
}

impl Runaways {
    /// Takes one reading and returns the app worth suggesting, if any: the busiest one that has
    /// stayed busy long enough, isn't snoozed, and isn't one you said never to suggest.
    pub fn add(&mut self, snapshot: &Snapshot, at: Instant, never: &[String], me: u32) -> Option<Suggestion> {
        let draining = snapshot.battery.as_ref().is_some_and(|battery| battery.status == "Discharging");
        let busy = |app: &App| app.cpu >= BUSY || (draining && app.watts.is_some_and(|watts| watts >= DRAIN_WATTS));
        self.busy_since.retain(|id, _| snapshot.apps.iter().any(|app| &app.id == id && busy(app)));
        for app in snapshot.apps.iter().filter(|app| busy(app)) {
            self.busy_since.entry(app.id.clone()).or_insert(at);
        }
        self.snoozed.retain(|_, until| *until > at);

        let app = snapshot
            .apps
            .iter()
            .filter(|app| self.busy_since.get(&app.id).is_some_and(|since| at.duration_since(*since) >= SUGGEST_AFTER))
            // Only apps you started, and never this one.
            .filter(|app| app.id.contains("/app.slice/") && !app.processes.iter().any(|process| process.pid == me))
            .filter(|app| !self.snoozed.contains_key(&app.id) && !never.contains(&key(app)))
            .max_by(|a, b| a.cpu.total_cmp(&b.cpu))?;
        let minutes = at.duration_since(self.busy_since[&app.id]).as_secs() / 60;
        let text = if app.cpu >= BUSY {
            let battery = if draining { " and is draining the battery" } else { "" };
            format!("{} has kept a core busy for {minutes} minutes{battery}.", app.name)
        } else {
            format!("{} has been draining the battery for {minutes} minutes.", app.name)
        };
        Some(Suggestion { app: app.id.clone(), name: app.name.clone(), key: key(app), text, pids: app.processes.iter().map(|process| process.pid).collect() })
    }

    pub fn snooze(&mut self, app: &str, at: Instant) {
        self.snoozed.insert(app.to_owned(), at + SNOOZE);
    }
}

fn key(app: &App) -> String {
    app.desktop_id.clone().unwrap_or_else(|| app.name.clone())
}

/// Asks the app to quit, as closing its windows would: a TERM to each of its processes.
pub fn quit(suggestion: &Suggestion) {
    let pids: Vec<String> = suggestion.pids.iter().map(u32::to_string).collect();
    if pids.is_empty() {
        return;
    }
    if let Err(err) = std::process::Command::new("kill").arg("-TERM").args(&pids).status() {
        eprintln!("notifyd: quitting {}: {err}", suggestion.name);
    }
}

pub enum Answer {
    Quit,
    NotNow,
    Never,
}

pub fn view(tasks: &Shared<Vec<Task>>, now: &Shared<glib::DateTime>, suggestion: &Shared<Option<Suggestion>>, on_answer: impl Fn(Answer) + 'static) -> gtk::Box {
    let face = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    face.add_css_class("kith-face");
    face.set_valign(gtk::Align::Center);
    face.set_halign(gtk::Align::Start);
    for _ in 0..2 {
        let eye = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        eye.add_css_class("eye");
        eye.set_valign(gtk::Align::Center);
        face.append(&eye);
    }
    let up_next = gtk::Label::builder().label("Up next").xalign(1.0).hexpand(true).build();
    up_next.add_css_class("detail");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(&face);
    header.append(&super::caption("KITH"));
    header.append(&up_next);

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let refresh_rows = {
        let (rows, now) = (rows.clone(), now.clone());
        move |tasks: &[Task]| {
            while let Some(child) = rows.first_child() {
                rows.remove(&child);
            }
            if tasks.is_empty() {
                let none = gtk::Label::builder().label("No upcoming tasks").xalign(0.0).build();
                none.add_css_class("detail");
                rows.append(&none);
            }
            let now = now.get();
            for task in tasks {
                let time = gtk::Label::builder().label(when(task.next, &now)).xalign(0.0).width_chars(6).build();
                time.add_css_class("when");
                let name = gtk::Label::builder().label(&task.name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row.append(&time);
                row.append(&name);
                rows.append(&row);
            }
        }
    };

    let text = gtk::Label::builder().xalign(0.0).wrap(true).build();
    text.add_css_class("suggestion-text");
    let quit = gtk::Button::with_label("Quit");
    quit.add_css_class("primary");
    let not_now = gtk::Button::with_label("Not now");
    not_now.add_css_class("secondary");
    let never = gtk::Button::builder().label("Don't suggest again").hexpand(true).halign(gtk::Align::End).build();
    never.add_css_class("quiet");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.append(&quit);
    buttons.append(&not_now);
    buttons.append(&never);
    let suggestion_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    suggestion_box.add_css_class("suggestion");
    suggestion_box.append(&text);
    suggestion_box.append(&buttons);

    let on_answer = std::rc::Rc::new(on_answer);
    for (button, answer) in [(&quit, 0), (&not_now, 1), (&never, 2)] {
        let on_answer = on_answer.clone();
        button.connect_clicked(move |_| {
            on_answer(match answer {
                0 => Answer::Quit,
                1 => Answer::NotNow,
                _ => Answer::Never,
            })
        });
    }

    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("card");
    card.add_css_class("widget");
    card.append(&header);
    card.append(&rows);
    card.append(&suggestion_box);

    let refresh = std::rc::Rc::new(refresh_rows);
    tasks.watch(&card, {
        let refresh = refresh.clone();
        move |_, tasks| refresh(tasks)
    });
    // "Sun" becomes "18:30" on the day.
    let tasks_now = tasks.clone();
    now.watch(&card, move |_, _| refresh(&tasks_now.get()));
    suggestion.watch(&card, move |_, suggestion| {
        suggestion_box.set_visible(suggestion.is_some());
        if let Some(suggestion) = suggestion {
            text.set_label(&suggestion.text);
            quit.set_label(&format!("Quit {}", suggestion.name));
        }
    });
    card
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

    fn snapshot(apps: Vec<App>, discharging: bool) -> Snapshot {
        let battery = discharging.then(|| Battery { status: "Discharging".to_owned(), ..Default::default() });
        Snapshot { apps, battery, ..Default::default() }
    }

    #[test]
    fn suggests_after_two_busy_minutes() {
        let mut runaways = Runaways::default();
        let start = Instant::now();
        let busy = snapshot(vec![app("kitchend", 150.0, None), app("editor", 3.0, None)], false);
        assert_eq!(runaways.add(&busy, start, &[], 1), None);
        assert_eq!(runaways.add(&busy, start + Duration::from_secs(60), &[], 1), None);
        let suggestion = runaways.add(&busy, start + Duration::from_secs(120), &[], 1).unwrap();
        assert_eq!(suggestion.name, "kitchend");
        assert_eq!(suggestion.text, "kitchend has kept a core busy for 2 minutes.");
        assert_eq!(suggestion.pids, vec![4242]);
    }

    #[test]
    fn a_quiet_reading_starts_over() {
        let mut runaways = Runaways::default();
        let start = Instant::now();
        let busy = snapshot(vec![app("kitchend", 150.0, None)], false);
        let quiet = snapshot(vec![app("kitchend", 10.0, None)], false);
        runaways.add(&busy, start, &[], 1);
        runaways.add(&quiet, start + Duration::from_secs(60), &[], 1);
        assert_eq!(runaways.add(&busy, start + Duration::from_secs(120), &[], 1), None);
    }

    #[test]
    fn battery_drain_counts_only_on_battery() {
        let start = Instant::now();
        let hungry = |discharging| snapshot(vec![app("renderer", 40.0, Some(9.0))], discharging);
        let mut plugged = Runaways::default();
        plugged.add(&hungry(false), start, &[], 1);
        assert_eq!(plugged.add(&hungry(false), start + Duration::from_secs(180), &[], 1), None);
        let mut unplugged = Runaways::default();
        unplugged.add(&hungry(true), start, &[], 1);
        let suggestion = unplugged.add(&hungry(true), start + Duration::from_secs(180), &[], 1).unwrap();
        assert_eq!(suggestion.text, "renderer has been draining the battery for 3 minutes.");
    }

    #[test]
    fn snoozed_never_and_own_apps_are_left_alone() {
        let start = Instant::now();
        let later = start + Duration::from_secs(120);
        let busy = snapshot(vec![app("kitchend", 150.0, None)], false);

        let mut snoozed = Runaways::default();
        snoozed.add(&busy, start, &[], 1);
        snoozed.snooze(&busy.apps[0].id, start);
        assert_eq!(snoozed.add(&busy, later, &[], 1), None);
        assert!(snoozed.add(&busy, start + SNOOZE + Duration::from_secs(1), &[], 1).is_some());

        let mut never = Runaways::default();
        never.add(&busy, start, &[], 1);
        assert_eq!(never.add(&busy, later, &["kitchend".to_owned()], 1), None);

        let mut mine = Runaways::default();
        mine.add(&busy, start, &[], 4242);
        assert_eq!(mine.add(&busy, later, &[], 4242), None);
    }

    #[test]
    fn task_names() {
        assert_eq!(task_name("autark-task@learning\\x2ddigest.timer", "autark-task@learning\\x2ddigest.timer"), "Learning digest");
        assert_eq!(task_name("autark-task@digest.timer", "Learning digest"), "Learning digest");
        assert_eq!(task_name("autark-task@weekly_review.timer", ""), "Weekly review");
    }
}
