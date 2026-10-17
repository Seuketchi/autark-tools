//! Kith's widget: its next scheduled tasks and its newest suggestion.
//!
//! Tasks are the `autark-task@<id>.timer` user timers of standalone mode; tasks that run on
//! Kith's own box arrive through `autark-ai` once it serves them. Suggestions are `autark-ai`'s,
//! read again whenever its conversation changes, and answered through it.

use autark_common::visor::{State, Visor};
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

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
pub trait Ai {
    fn open_suggestions(&self) -> zbus::Result<String>;
    fn answer_suggestion(&self, entry: u64, answer: &str) -> zbus::Result<String>;
    #[zbus(property(emits_changed_signal = "false"))]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(signal)]
    fn state_changed(&self, state: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn history_changed(&self) -> zbus::Result<()>;
}

/// Kith's newest open suggestion, as `autark-ai` keeps it in the conversation.
#[derive(Clone, PartialEq, Debug)]
pub struct Suggestion {
    pub entry: u64,
    pub text: String,
    /// The offer's button, empty when it offers nothing.
    pub offer: String,
}

/// The newest of `OpenSuggestions`' entries.
pub fn newest(json: &str) -> Option<Suggestion> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let entry = entries.last()?;
    Some(Suggestion {
        entry: entry["id"].as_u64()?,
        text: entry["text"].as_str().unwrap_or_default().to_owned(),
        offer: if entry["offer"].is_null() { String::new() } else { entry["offer_label"].as_str().unwrap_or_default().to_owned() },
    })
}

#[derive(Clone, Copy)]
pub enum Answer {
    Accept,
    NotNow,
    Never,
}

impl Answer {
    pub fn word(self) -> &'static str {
        match self {
            Answer::Accept => "accept",
            Answer::NotNow => "not_now",
            Answer::Never => "never",
        }
    }
}

pub fn view(tasks: &Shared<Vec<Task>>, now: &Shared<glib::DateTime>, suggestion: &Shared<Option<Suggestion>>, kith: &Shared<State>, on_answer: impl Fn(Answer) + 'static) -> gtk::Box {
    let face = Visor::new(24);
    let up_next = gtk::Label::builder().label("Up next").xalign(1.0).hexpand(true).build();
    up_next.add_css_class("detail");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(face.widget());
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
    let offer = gtk::Button::new();
    offer.add_css_class("primary");
    let not_now = gtk::Button::with_label("Not now");
    not_now.add_css_class("secondary");
    let never = gtk::Button::builder().label("Don't suggest again").hexpand(true).halign(gtk::Align::End).build();
    never.add_css_class("quiet");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.append(&offer);
    buttons.append(&not_now);
    buttons.append(&never);
    let suggestion_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    suggestion_box.add_css_class("suggestion");
    suggestion_box.append(&text);
    suggestion_box.append(&buttons);

    let on_answer = std::rc::Rc::new(on_answer);
    for (button, answer) in [(&offer, Answer::Accept), (&not_now, Answer::NotNow), (&never, Answer::Never)] {
        let on_answer = on_answer.clone();
        button.connect_clicked(move |_| on_answer(answer));
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
    kith.watch(&card, move |_, state| face.set_state(*state));
    suggestion.watch(&card, move |_, suggestion| {
        suggestion_box.set_visible(suggestion.is_some());
        if let Some(suggestion) = suggestion {
            text.set_label(&suggestion.text);
            offer.set_label(&suggestion.offer);
            offer.set_visible(!suggestion.offer.is_empty());
        }
    });
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_open_suggestion() {
        let json = r#"[{"id":4,"kind":"suggestion","text":"Old.","offer":null,"offer_label":""},
                       {"id":9,"kind":"suggestion","text":"kitchend has kept a core busy for 14 minutes. Quit it?","offer":{"action":"quit_app","app":"/x"},"offer_label":"Quit kitchend"}]"#;
        assert_eq!(newest(json), Some(Suggestion { entry: 9, text: "kitchend has kept a core busy for 14 minutes. Quit it?".into(), offer: "Quit kitchend".into() }));
        assert_eq!(newest("[]"), None);
    }
}
