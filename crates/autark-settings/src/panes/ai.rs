//! AI: the switch that turns `autark-ai` on, local-only mode, the Ollama model (listed by `autark-ai`'s
//! `Models`), Jev's API key (in the keyring) and which features Jev answers, all in `ai.toml`, which
//! `autark-ai` rereads; Kith's suggestions, with what you said never to suggest again; and the last calls from
//! the call log. Until Jev's client is built, Ollama answers every feature.

use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::{bus, config, secrets, ui};

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
trait Ai {
    fn models(&self) -> zbus::Result<Vec<String>>;
    fn never_suggested(&self) -> zbus::Result<Vec<(String, String)>>;
    fn suggest_again(&self, key: &str) -> zbus::Result<()>;
}

/// Jev's features as `ai.toml`'s `[jev]` names them, with the design's words.
const FEATURES: [(&str, &str, &str); 3] = [
    ("launcher", "Launcher commands", "Turns what you type into actions"),
    ("notify", "Notification urgency", "Focus lets only urgent ones through"),
    ("terminal", "Terminal safety check", "Confirms destructive commands"),
];

/// The call log's last rows shown.
const LOG_ROWS: usize = 5;

/// What the keyring said about Jev's key.
#[derive(Clone)]
enum Key {
    Unknown,
    Stored,
    Missing,
    Unreachable(String),
}

struct AiPane {
    top: gtk::Box,
    features: gtk::Box,
    suggestions: gtk::Box,
    log: gtk::Box,
    status: gtk::Label,
    models: RefCell<Result<Vec<String>, String>>,
    key: RefCell<Key>,
    /// What's never suggested again: each one's key and name.
    nevers: RefCell<Result<Vec<(String, String)>, String>>,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("AI", Some("Ollama runs on this laptop. Jev runs in the cloud, and you turn it on one feature at a time.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let ai = Rc::new(AiPane {
        top: ui::group(),
        features: gtk::Box::new(gtk::Orientation::Vertical, 0),
        suggestions: gtk::Box::new(gtk::Orientation::Vertical, 0),
        log: gtk::Box::new(gtk::Orientation::Vertical, 0),
        status: status.clone(),
        models: RefCell::new(Ok(Vec::new())),
        key: RefCell::new(Key::Unknown),
        nevers: RefCell::new(Ok(Vec::new())),
    });
    for part in [&ai.top, &ai.features, &ai.suggestions, &ai.log] {
        column.append(part);
    }
    column.append(&status);
    page.connect_map({
        let ai = ai.clone();
        move |_| ai.refresh()
    });
    page.upcast()
}

impl AiPane {
    /// Everything afresh: the files at once, then what each daemon says as it answers, none waiting on another.
    fn refresh(self: &Rc<Self>) {
        self.render_top();
        self.render_log();
        self.render_suggestions();
        let ai = self.clone();
        glib::spawn_future_local(async move { ai.load_nevers().await });
        let ai = self.clone();
        glib::spawn_future_local(async move {
            let models = async { AiProxy::new(&bus::session().await?).await?.models().await };
            *ai.models.borrow_mut() = models.await.map_err(|error| bus::message(error, "autark-ai"));
            ai.render_top();
        });
        let ai = self.clone();
        glib::spawn_future_local(async move {
            let asked = Box::pin(async { secrets::has_jev_key(&bus::session().await?).await });
            // A keyring that D-Bus is still starting (or can't) shouldn't leave the row waiting.
            let late = Box::pin(glib::timeout_future(Duration::from_secs(10)));
            *ai.key.borrow_mut() = match futures_util::future::select(asked, late).await {
                futures_util::future::Either::Left((Ok(true), _)) => Key::Stored,
                futures_util::future::Either::Left((Ok(false), _)) => Key::Missing,
                futures_util::future::Either::Left((Err(error), _)) => Key::Unreachable(bus::message(error, secrets::DAEMON)),
                futures_util::future::Either::Right(_) => Key::Unreachable("The keyring didn’t answer".to_owned()),
            };
            ai.render_top();
        });
    }

    fn report(&self, result: Result<(), String>) {
        self.status.set_visible(result.is_err());
        if let Err(error) = result {
            self.status.set_label(&error);
        }
    }

    /// Sets one key of `ai.toml` and redraws what depends on it.
    fn set(self: &Rc<Self>, key: &'static str, value: impl Into<toml_edit::Value> + 'static) {
        self.report(config::edit("ai", |document| config::set_in(document, key, value)));
        self.render_top();
        self.render_suggestions();
    }

    fn render_top(self: &Rc<Self>) {
        ui::rebuild_keeping_focus(&[&self.top, &self.features], || self.build_top());
    }

    fn build_top(self: &Rc<Self>) {
        ui::clear(&self.top);
        ui::clear(&self.features);
        let document = config::read("ai");
        let enabled = config::get_bool(&document, "enabled").unwrap_or(false);
        let local = config::get_bool(&document, "local_only").unwrap_or(false);

        let ai = self.clone();
        let on = ui::switch(enabled, move |on| ai.set("enabled", on));
        on.set_widget_name("ai:enabled");
        on.update_property(&[gtk::accessible::Property::Label("AI")]);
        let note = if enabled { "Answers in the launcher and the terminal" } else { "Off: nothing is asked of a model. Destructive commands are still checked by rule" };
        self.top.append(&ui::row("AI", Some(note), &on));

        let ai = self.clone();
        let local_switch = ui::switch(local, move |on| ai.set("local_only", on));
        local_switch.set_widget_name("ai:local");
        local_switch.update_property(&[gtk::accessible::Property::Label("Local-only mode")]);
        let row = ui::row("Local-only mode", Some("Everything goes to Ollama. Turns on by itself when you’re offline."), &local_switch);
        row.set_sensitive(enabled);
        self.top.append(&row);

        let current = config::get_str(&document, "ollama.model").unwrap_or_default();
        let (mut names, note) = match &*self.models.borrow() {
            Ok(names) => (names.clone(), "Running on this laptop".to_owned()),
            Err(error) => (Vec::new(), format!("Couldn’t list the models: {error}")),
        };
        if !current.is_empty() && !names.contains(&current) {
            names.insert(0, current.clone());
        }
        let row = if names.is_empty() {
            let none = ui::note("None pulled");
            let note = if self.models.borrow().is_ok() { "Pull one with “ollama pull”, then choose it here".to_owned() } else { note };
            ui::row("Ollama model", Some(&note), &none)
        } else {
            let options: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), name.as_str())).collect();
            let ai = self.clone();
            let model = ui::choice(&options, &current, move |name| {
                let name = name.to_owned();
                ai.report(config::edit("ai", |document| config::set_in(document, "ollama.model", name)));
            });
            // Nothing chosen yet: the drop-down shows the first, which isn't in effect until picked.
            if current.is_empty() {
                model.set_selected(gtk::INVALID_LIST_POSITION);
            }
            model.set_widget_name("ai:model");
            model.update_property(&[gtk::accessible::Property::Label("Ollama model")]);
            ui::row("Ollama model", Some(&note), &model)
        };
        row.set_sensitive(enabled);
        self.top.append(&row);

        let count = FEATURES.iter().filter(|(id, ..)| config::get_bool(&document, &format!("jev.{id}")).unwrap_or(false)).count();
        let usage = if local { "Off while local-only mode is on".to_owned() } else { format!("On for {count} of {} features", FEATURES.len()) };
        let (note, label) = match &*self.key.borrow() {
            Key::Stored => (format!("{usage} · API key stored in the keyring"), "Change key"),
            Key::Missing => (format!("{usage} · No API key yet"), "Add key"),
            Key::Unknown => (usage, "Change key"),
            Key::Unreachable(error) => (format!("{usage} · {error}"), "Add key"),
        };
        let ai = self.clone();
        let change = ui::button(label, move |button| ai.ask_key(button.root().and_downcast::<gtk::Window>().as_ref()));
        change.set_widget_name("ai:key");
        change.set_sensitive(!matches!(*self.key.borrow(), Key::Unreachable(_)));
        let row = ui::row("Jev", Some(&note), &change);
        row.set_sensitive(enabled);
        self.top.append(&row);

        let group = ui::group();
        for (id, label, description) in FEATURES {
            let key = format!("jev.{id}");
            let on = config::get_bool(&document, &key).unwrap_or(false);
            let ai = self.clone();
            let switch = ui::switch(on && !local, move |on| {
                let key = key.clone();
                ai.report(config::edit("ai", |document| config::set_in(document, &key, on)));
                ai.render_top();
            });
            switch.set_widget_name(&format!("ai:jev:{id}"));
            switch.update_property(&[gtk::accessible::Property::Label(label)]);
            let row = ui::row(label, Some(description), &switch);
            row.set_sensitive(enabled && !local);
            group.append(&row);
        }
        let aside = ui::note("Ollama answers these until Jev’s client is built");
        self.features.append(&ui::section("Use Jev for", Some(&aside), &group));
    }

    /// A dialog for Jev's API key, which goes straight to the keyring.
    fn ask_key(self: &Rc<Self>, parent: Option<&gtk::Window>) {
        let window = gtk::Window::builder().title("Jev API key").modal(true).resizable(false).default_width(380).build();
        window.set_transient_for(parent);
        window.set_application(parent.and_then(|parent| parent.application()).as_ref());
        window.add_css_class("settings");
        window.add_css_class("dialog");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.add_css_class("page");
        let explain = ui::note("From your TypeSafe account. It’s kept in the keyring, where only autark-ai reads it.");
        explain.set_wrap(true);
        column.append(&explain);
        let entry = gtk::PasswordEntry::builder().placeholder_text("API key").show_peek_icon(true).build();
        entry.add_css_class("field");
        entry.update_property(&[gtk::accessible::Property::Label("API key")]);
        column.append(&entry);
        let problem = ui::note("");
        problem.add_css_class("error");
        problem.set_wrap(true);
        problem.set_visible(false);
        column.append(&problem);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let cancel = ui::button("Cancel", {
            let window = window.downgrade();
            move |_| {
                if let Some(window) = window.upgrade() {
                    window.close();
                }
            }
        });
        let save = ui::button("Save", |_| {});
        save.add_css_class("suggested");
        buttons.append(&cancel);
        buttons.append(&save);
        column.append(&buttons);
        window.set_child(Some(&column));

        let submit = {
            let (ai, window, entry, problem, save) = (self.clone(), window.downgrade(), entry.clone(), problem.clone(), save.clone());
            move || {
                let key = Zeroizing::new(entry.text().trim().to_owned());
                if key.is_empty() {
                    problem.set_label("Paste the key first.");
                    problem.set_visible(true);
                    return;
                }
                save.set_sensitive(false);
                problem.set_visible(false);
                let (ai, window, problem, save) = (ai.clone(), window.clone(), problem.clone(), save.clone());
                glib::spawn_future_local(async move {
                    let result = match bus::session().await {
                        Ok(connection) => secrets::store_jev_key(&connection, key).await.map_err(|error| bus::message(error, secrets::DAEMON)),
                        Err(error) => Err(error.to_string()),
                    };
                    save.set_sensitive(true);
                    match result {
                        Ok(true) => {
                            *ai.key.borrow_mut() = Key::Stored;
                            ai.render_top();
                            if let Some(window) = window.upgrade() {
                                window.close();
                            }
                        }
                        Ok(false) => {
                            problem.set_label("The keyring stayed locked, so the key wasn’t saved.");
                            problem.set_visible(true);
                        }
                        Err(error) => {
                            problem.set_label(&format!("Couldn’t save the key: {error}"));
                            problem.set_visible(true);
                        }
                    }
                });
            }
        };
        let submit = Rc::new(submit);
        save.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        entry.connect_activate(move |_| submit());
        window.present();
    }

    async fn load_nevers(self: &Rc<Self>) {
        let nevers = async { AiProxy::new(&bus::session().await?).await?.never_suggested().await };
        *self.nevers.borrow_mut() = nevers.await.map_err(|error| bus::message(error, "autark-ai"));
        self.render_suggestions();
    }

    fn render_suggestions(self: &Rc<Self>) {
        ui::rebuild_keeping_focus(&[&self.suggestions], || self.build_suggestions());
    }

    fn build_suggestions(self: &Rc<Self>) {
        ui::clear(&self.suggestions);
        let document = config::read("ai");
        let enabled = config::get_bool(&document, "enabled").unwrap_or(false);
        let on = config::get_bool(&document, "features.suggest").unwrap_or(true);
        let group = ui::group();
        let ai = self.clone();
        let switch = ui::switch(on, move |on| {
            ai.report(config::edit("ai", |document| config::set_in(document, "features.suggest", on)));
            ai.render_suggestions();
        });
        switch.set_widget_name("ai:suggest");
        switch.update_property(&[gtk::accessible::Property::Label("Suggestions")]);
        let row = ui::row("Suggestions", Some("A card when Kith notices something worth a look: an app running away, the battery low, a build ending, a new network. Never a banner"), &switch);
        row.set_sensitive(enabled);
        group.append(&row);
        let nevers = self.nevers.borrow().clone();
        match nevers {
            Err(error) => group.append(&empty_row(&format!("What’s never suggested comes from autark-ai, and {error}"))),
            Ok(nevers) => {
                for (key, label) in nevers {
                    let ai = self.clone();
                    let again = ui::button("Suggest again", move |_| {
                        let (ai, key) = (ai.clone(), key.clone());
                        glib::spawn_future_local(async move {
                            let result = async { AiProxy::new(&bus::session().await?).await?.suggest_again(&key).await };
                            ai.report(result.await.map_err(|error| bus::message(error, "autark-ai")));
                            ai.load_nevers().await;
                        });
                    });
                    again.update_property(&[gtk::accessible::Property::Label(&format!("Suggest again: {label}"))]);
                    group.append(&ui::row(&label, Some("Never suggested again"), &again));
                }
            }
        }
        let per_hour = config::get_int(&document, "suggestions.per_hour").unwrap_or(3);
        let aside = ui::note(&format!("At most {per_hour} an hour, rated by Ollama"));
        self.suggestions.append(&ui::section("Suggestions", Some(&aside), &group));
    }

    fn render_log(self: &Rc<Self>) {
        ui::clear(&self.log);
        let path = log_path();
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some("Call log"));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        heading.set_hexpand(true);
        top.append(&heading);
        let open = ui::button(&format!("Open {}", home_relative(&path)), {
            let path = path.clone();
            move |button| {
                let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
                launcher.launch(button.root().and_downcast::<gtk::Window>().as_ref(), gio::Cancellable::NONE, |result| {
                    if let Err(error) = result {
                        eprintln!("settings: opening the AI call log: {error}");
                    }
                });
            }
        });
        open.set_sensitive(path.exists());
        top.append(&open);
        let group = ui::group();
        let rows = last_calls(&path, LOG_ROWS);
        if rows.is_empty() {
            group.append(&empty_row("No calls yet. Each one is logged here with what was asked and answered."));
        } else {
            let grid = gtk::Grid::builder().column_spacing(16).row_spacing(8).build();
            grid.add_css_class("row");
            grid.add_css_class("calls");
            for (column, title) in ["Time", "Feature", "Engine", "Answer", "Score"].into_iter().enumerate() {
                let label = ui::note(&title.to_uppercase());
                label.add_css_class("column-heading");
                grid.attach(&label, column as i32, 0, 1, 1);
            }
            let now = glib::DateTime::now_local().ok();
            for (row, call) in rows.iter().enumerate() {
                let time = now.as_ref().map(|now| when(call.time * 1_000_000, now)).unwrap_or_default();
                let cells = [time, call.feature.clone(), call.engine.clone(), call.answer.clone(), call.score.clone()];
                for (column, text) in cells.into_iter().enumerate() {
                    let label = gtk::Label::new(Some(&text));
                    label.set_xalign(0.0);
                    if column == 3 {
                        label.set_hexpand(true);
                        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                        label.set_max_width_chars(40);
                        label.set_tooltip_text(Some(&text));
                    }
                    if column == 2 && text == "Ollama" {
                        label.add_css_class("good");
                    }
                    if column == 0 || column == 4 {
                        label.add_css_class("code");
                    }
                    grid.attach(&label, column as i32, row as i32 + 1, 1, 1);
                }
            }
            grid.update_property(&[gtk::accessible::Property::Label("Recent AI calls")]);
            group.append(&grid);
        }
        let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
        column.append(&top);
        column.append(&group);
        self.log.append(&column);
    }
}

fn empty_row(text: &str) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    line.add_css_class("row");
    let label = ui::note(text);
    label.set_wrap(true);
    label.set_hexpand(true);
    line.append(&label);
    line
}

/// A path as the design writes it, with `~` for home.
fn home_relative(path: &std::path::Path) -> String {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// "18:30" today, "Sun" this week, "24 Oct" otherwise, for microseconds since the epoch.
fn when(micros: u64, now: &glib::DateTime) -> String {
    let Ok(time) = glib::DateTime::from_unix_local((micros / 1_000_000) as i64) else { return String::new() };
    let days = (time.difference(now).as_seconds() / 86_400).abs();
    let format = if time.day_of_year() == now.day_of_year() && time.year() == now.year() {
        "%H:%M"
    } else if days < 6 {
        "%a %H:%M"
    } else {
        "%-d %b"
    };
    time.format(format).map(String::from).unwrap_or_default()
}

fn log_path() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/state")).join("autark-ai/log")
}

/// One line of the call log, in the table's words.
#[derive(Debug, PartialEq)]
struct Call {
    /// Seconds since the epoch.
    time: u64,
    feature: String,
    engine: String,
    answer: String,
    score: String,
}

/// The last `count` calls, newest first, read from the end of the log only.
fn last_calls(path: &std::path::Path, count: usize) -> Vec<Call> {
    let Ok(mut file) = std::fs::File::open(path) else { return Vec::new() };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let start = length.saturating_sub(64 * 1024);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut tail = String::new();
    if file.read_to_string(&mut tail).is_err() {
        return Vec::new();
    }
    // A cut first line is only part of one.
    let lines = tail.lines().skip(usize::from(start > 0));
    let mut calls: Vec<Call> = lines.filter_map(|line| serde_json::from_str::<Value>(line).ok()).map(|line| call(&line)).collect();
    calls.reverse();
    calls.truncate(count);
    calls
}

fn call(line: &Value) -> Call {
    let answer = &line["answer"];
    let feature = match line["call"].as_str().unwrap_or_default() {
        "route" => "Launcher",
        "check" => "Terminal",
        "ask" => "Answer",
        other => other,
    };
    let engine = match line["engine"].as_str().unwrap_or_default() {
        "ollama" => "Ollama".to_owned(),
        "jev" => "Jev".to_owned(),
        "rules" => "Rules".to_owned(),
        other => other.to_owned(),
    };
    let score = |value: &Value| value.as_f64().map_or_else(|| "n/a".to_owned(), |value| format!("{value:.2}"));
    let (text, score) = if let Some(error) = line["error"].as_str() {
        (format!("Failed: {error}"), "n/a".to_owned())
    } else {
        match line["call"].as_str().unwrap_or_default() {
            "route" => {
                let text = match answer["kind"].as_str() {
                    Some("actions") => answer["actions"].as_array().into_iter().flatten().map(action).collect::<Vec<_>>().join(", "),
                    Some("search") => format!("Search: {}", answer["query"].as_str().unwrap_or_default()),
                    Some("question") => "A question, for Ask".to_owned(),
                    _ => answer.to_string(),
                };
                (text, score(&answer["confidence"]))
            }
            "check" => (format!("{}: {}", line["input"].as_str().unwrap_or_default(), answer["reason"].as_str().unwrap_or("no risk seen")), score(&answer["risk"])),
            _ => (answer.as_str().map_or_else(|| answer.to_string(), |text| text.split_whitespace().collect::<Vec<_>>().join(" ")), "n/a".to_owned()),
        }
    };
    Call { time: line["time"].as_u64().unwrap_or(0), feature: feature.to_owned(), engine, answer: text, score }
}

/// `set_volume(30)`, `mute(on)`, `open_app("Files")`.
fn action(action: &Value) -> String {
    let name = action["name"].as_str().unwrap_or("?");
    let argument = if let Some(number) = action["number"].as_f64() {
        format!("{number}")
    } else if let Some(on) = action["on"].as_bool() {
        (if on { "on" } else { "off" }).to_owned()
    } else if let Some(text) = action["text"].as_str() {
        format!("“{text}”")
    } else {
        String::new()
    };
    format!("{name}({argument})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn log_lines_read_as_table_rows() {
        let route = json!({ "time": 5, "call": "route", "engine": "ollama", "input": "dim and mute", "answer": { "kind": "actions", "actions": [{ "name": "set_brightness", "number": 30 }, { "name": "mute", "on": true }], "confidence": 0.94 }, "error": null });
        assert_eq!(call(&route), Call { time: 5, feature: "Launcher".into(), engine: "Ollama".into(), answer: "set_brightness(30), mute(on)".into(), score: "0.94".into() });
        let check = json!({ "time": 6, "call": "check", "engine": "rules", "input": "rm -rf ~", "answer": { "risk": 1.0, "reason": "deletes your home" }, "error": null });
        assert_eq!(call(&check).answer, "rm -rf ~: deletes your home");
        let failed = json!({ "time": 7, "call": "ask", "engine": "ollama", "input": {}, "answer": null, "error": "Ollama at http://localhost:11434: refused" });
        assert_eq!(call(&failed).answer, "Failed: Ollama at http://localhost:11434: refused");
    }
}
