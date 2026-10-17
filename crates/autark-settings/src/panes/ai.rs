//! AI: the switch that turns `autark-ai` on, local-only mode, the Ollama model (listed by `autark-ai`'s
//! `Models`), Jev's API key (in the keyring) and which features Jev answers, all in `ai.toml`, which
//! `autark-ai` rereads; Kith's tasks (systemd user timers); what the assistant remembers (`memory.toml`); and
//! the last calls from the call log. Until Jev's client is built, Ollama answers every feature.

use std::cell::{Cell, RefCell};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::memory::{self, Fact};
use crate::tasks::{self, Task};
use crate::{bus, config, secrets, ui};

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
trait Ai {
    fn models(&self) -> zbus::Result<Vec<String>>;
}

/// Jev's features as `ai.toml`'s `[jev]` names them, with the design's words.
const FEATURES: [(&str, &str, &str); 5] = [
    ("launcher", "Launcher commands", "Turns what you type into actions"),
    ("notify", "Notification urgency", "Focus lets only urgent ones through"),
    ("terminal", "Terminal safety check", "Confirms destructive commands"),
    ("files", "File search by meaning", "Ranks files by relevance"),
    ("settings", "Settings search", "Finds the right pane from a sentence"),
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
    tasks: gtk::Box,
    memory: gtk::Box,
    log: gtk::Box,
    status: gtk::Label,
    models: RefCell<Result<Vec<String>, String>>,
    key: RefCell<Key>,
    task_list: RefCell<Result<Vec<Task>, String>>,
    /// Tasks started with Run now that systemd hasn't reported running yet.
    starting: RefCell<Vec<String>>,
    /// The fact being corrected, by position, and the text typed so far.
    editing: RefCell<Option<(usize, Fact)>>,
    draft: RefCell<String>,
    shown: Cell<bool>,
    ticks: Cell<u32>,
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
        tasks: gtk::Box::new(gtk::Orientation::Vertical, 0),
        memory: gtk::Box::new(gtk::Orientation::Vertical, 0),
        log: gtk::Box::new(gtk::Orientation::Vertical, 0),
        status: status.clone(),
        models: RefCell::new(Ok(Vec::new())),
        key: RefCell::new(Key::Unknown),
        task_list: RefCell::new(Ok(Vec::new())),
        starting: RefCell::default(),
        editing: RefCell::default(),
        draft: RefCell::default(),
        shown: Cell::new(false),
        ticks: Cell::new(0),
    });
    for part in [&ai.top, &ai.features, &ai.tasks, &ai.memory, &ai.log] {
        column.append(part);
    }
    column.append(&status);
    page.connect_map({
        let ai = ai.clone();
        move |_| {
            ai.shown.set(true);
            ai.refresh();
        }
    });
    page.connect_unmap({
        let ai = ai.clone();
        move |_| ai.shown.set(false)
    });
    // Tasks: often while one runs, so Run now's label comes back; otherwise every half minute for "next".
    let weak = Rc::downgrade(&ai);
    glib::timeout_add_local(Duration::from_secs(2), move || {
        let Some(ai) = weak.upgrade() else { return glib::ControlFlow::Break };
        if ai.shown.get() {
            let ticks = ai.ticks.get() + 1;
            ai.ticks.set(ticks);
            let running = ai.task_list.borrow().as_ref().is_ok_and(|tasks| tasks.iter().any(|task| task.running)) || !ai.starting.borrow().is_empty();
            if running || ticks % 15 == 0 {
                glib::spawn_future_local({
                    let ai = ai.clone();
                    async move { ai.load_tasks().await }
                });
            }
        }
        glib::ControlFlow::Continue
    });
    page.upcast()
}

impl AiPane {
    /// Everything afresh: the files at once, then what each daemon says as it answers, none waiting on another.
    fn refresh(self: &Rc<Self>) {
        self.render_top();
        self.render_memory();
        self.render_log();
        self.render_tasks();
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
        let ai = self.clone();
        glib::spawn_future_local(async move { ai.load_tasks().await });
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
        let note = if enabled { "Answers in the launcher, the terminal and search" } else { "Off: nothing is asked of a model. Destructive commands are still checked by rule" };
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

    async fn load_tasks(self: &Rc<Self>) {
        let list = match bus::session().await {
            Ok(connection) => tasks::list(&connection).await.map_err(|error| bus::message(error, tasks::DAEMON)),
            Err(error) => Err(error.to_string()),
        };
        if let Ok(list) = &list {
            self.starting.borrow_mut().retain(|id| list.iter().any(|task| task.id == *id && !task.running));
        }
        *self.task_list.borrow_mut() = list;
        self.render_tasks();
    }

    fn render_tasks(self: &Rc<Self>) {
        ui::rebuild_keeping_focus(&[&self.tasks], || self.build_tasks());
    }

    fn build_tasks(self: &Rc<Self>) {
        ui::clear(&self.tasks);
        let group = ui::group();
        let list = self.task_list.borrow().clone();
        match list {
            Err(error) => group.append(&empty_row(&format!("Tasks are systemd user timers, and {error}"))),
            Ok(list) if list.is_empty() => group.append(&empty_row("No tasks. Ask the assistant to do something on a schedule.")),
            Ok(list) => {
                let now = glib::DateTime::now_local().ok();
                for task in list {
                    let schedule = task.calendar.as_deref().map(tasks::schedule).unwrap_or_else(|| "No schedule".to_owned());
                    let next = if !task.on { "paused".to_owned() } else { now.as_ref().map(|now| format!("next: {}", when(task.next, now))).unwrap_or_default() };
                    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                    line.add_css_class("row");
                    line.append(&texts(&task.name, &format!("{schedule} · {next}")));
                    let running = task.running || self.starting.borrow().contains(&task.id);
                    let ai = self.clone();
                    let started = task.clone();
                    let run = ui::button(if running { "Running…" } else { "Run now" }, move |_| ai.run_task(started.clone()));
                    run.set_sensitive(!running);
                    run.set_widget_name(&format!("ai:run:{}", task.id));
                    run.update_property(&[gtk::accessible::Property::Label(&format!("Run {} now", task.name))]);
                    line.append(&run);
                    let ai = self.clone();
                    let toggled = task.clone();
                    let switch = ui::switch(task.on, move |on| {
                        let (ai, task) = (ai.clone(), toggled.clone());
                        glib::spawn_future_local(async move {
                            let result = async { tasks::set_on(&bus::session().await?, &task, on).await };
                            ai.report(result.await.map_err(|error| bus::message(error, tasks::DAEMON)));
                            ai.load_tasks().await;
                        });
                    });
                    switch.set_widget_name(&format!("ai:task:{}", task.id));
                    switch.update_property(&[gtk::accessible::Property::Label(&task.name)]);
                    line.append(&switch);
                    let ai = self.clone();
                    let doomed = task.clone();
                    let delete = ui::button("Delete", move |_| {
                        let (ai, task) = (ai.clone(), doomed.clone());
                        glib::spawn_future_local(async move {
                            let result = match bus::session().await {
                                Ok(connection) => tasks::delete(&connection, &task).await,
                                Err(error) => Err(error.to_string()),
                            };
                            ai.report(result);
                            ai.load_tasks().await;
                        });
                    });
                    delete.add_css_class("danger");
                    delete.set_widget_name(&format!("ai:delete:{}", task.id));
                    delete.update_property(&[gtk::accessible::Property::Label(&format!("Delete task: {}", task.name))]);
                    line.append(&delete);
                    group.append(&line);
                }
            }
        }
        let aside = ui::note("Scheduled prompts, run on this laptop");
        self.tasks.append(&ui::section("Tasks", Some(&aside), &group));
    }

    fn run_task(self: &Rc<Self>, task: Task) {
        self.starting.borrow_mut().push(task.id.clone());
        self.render_tasks();
        let ai = self.clone();
        glib::spawn_future_local(async move {
            let result = async { tasks::run(&bus::session().await?, &task).await };
            let result = result.await.map_err(|error| bus::message(error, tasks::DAEMON));
            if result.is_err() {
                ai.starting.borrow_mut().retain(|id| *id != task.id);
            }
            ai.report(result);
            ai.load_tasks().await;
        });
    }

    fn render_memory(self: &Rc<Self>) {
        ui::rebuild_keeping_focus(&[&self.memory], || self.build_memory());
    }

    fn build_memory(self: &Rc<Self>) {
        ui::clear(&self.memory);
        let facts = memory::read();
        // A fact corrected elsewhere meanwhile ends the edit.
        let editing = self.editing.borrow().clone().filter(|(index, fact)| facts.get(*index) == Some(fact));
        *self.editing.borrow_mut() = editing.clone();
        let group = ui::group();
        let today = glib::DateTime::now_local().ok();
        for (index, fact) in facts.iter().enumerate() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            line.add_css_class("row");
            if editing.as_ref().is_some_and(|(at, _)| *at == index) {
                let entry = gtk::Entry::builder().text(self.draft.borrow().as_str()).hexpand(true).build();
                entry.add_css_class("field");
                entry.set_widget_name("ai:memory:edit");
                entry.update_property(&[gtk::accessible::Property::Label("Edit memory")]);
                entry.connect_changed({
                    let ai = self.clone();
                    move |entry| *ai.draft.borrow_mut() = entry.text().to_string()
                });
                let save = {
                    let (ai, fact) = (self.clone(), fact.clone());
                    move || {
                        let text = ai.draft.borrow().trim().to_owned();
                        let result = if text.is_empty() { memory::forget(index, &fact) } else { memory::correct(index, &fact, &text) };
                        ai.editing.borrow_mut().take();
                        ai.report(result);
                        ai.render_memory();
                        ui::focus_named(&ai.memory, &format!("ai:memory:edit:{index}"));
                    }
                };
                let save = Rc::new(save);
                entry.connect_activate({
                    let save = save.clone();
                    move |_| save()
                });
                line.append(&entry);
                let button = ui::button("Save", move |_| save());
                button.set_widget_name("ai:memory:save");
                line.append(&button);
                let ai = self.clone();
                let cancel = ui::button("Cancel", move |_| {
                    ai.editing.borrow_mut().take();
                    ai.render_memory();
                    ui::focus_named(&ai.memory, &format!("ai:memory:edit:{index}"));
                });
                cancel.set_widget_name("ai:memory:cancel");
                line.append(&cancel);
                glib::idle_add_local_once({
                    let entry = entry.downgrade();
                    move || {
                        if let Some(entry) = entry.upgrade().filter(|entry| !entry.has_focus()) {
                            entry.grab_focus_without_selecting();
                            entry.set_position(-1);
                        }
                    }
                });
            } else {
                let text = gtk::Label::new(Some(&fact.text));
                text.add_css_class("row-label");
                text.set_xalign(0.0);
                text.set_hexpand(true);
                text.set_wrap(true);
                line.append(&text);
                line.append(&ui::note(&added(fact.added.as_deref(), today.as_ref())));
                let ai = self.clone();
                let corrected = fact.clone();
                let edit = ui::button("Edit", move |_| {
                    *ai.draft.borrow_mut() = corrected.text.clone();
                    *ai.editing.borrow_mut() = Some((index, corrected.clone()));
                    ai.render_memory();
                });
                edit.set_widget_name(&format!("ai:memory:edit:{index}"));
                edit.update_property(&[gtk::accessible::Property::Label(&format!("Edit memory: {}", fact.text))]);
                line.append(&edit);
                let ai = self.clone();
                let forgotten = fact.clone();
                let forget = ui::button("Forget", move |_| {
                    ai.report(memory::forget(index, &forgotten));
                    ai.render_memory();
                });
                forget.add_css_class("danger");
                forget.set_widget_name(&format!("ai:memory:forget:{index}"));
                forget.update_property(&[gtk::accessible::Property::Label(&format!("Forget memory: {}", fact.text))]);
                line.append(&forget);
            }
            group.append(&line);
        }
        if facts.is_empty() {
            group.append(&empty_row("No memories. The assistant asks before saving anything."));
        }
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let path = ui::note(&home_relative(&memory::path()));
        path.add_css_class("code");
        path.set_hexpand(true);
        path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        foot.append(&path);
        let ai = self.clone();
        let all = ui::button("Forget all", move |_| {
            ai.editing.borrow_mut().take();
            ai.report(memory::forget_all());
            ai.render_memory();
        });
        all.add_css_class("danger");
        all.set_sensitive(!facts.is_empty());
        all.set_widget_name("ai:memory:all");
        foot.append(&all);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.append(&group);
        body.append(&foot);
        let summary = format!("{} {}, each approved by you", facts.len(), if facts.len() == 1 { "fact" } else { "facts" });
        let aside = (!facts.is_empty()).then(|| ui::note(&summary));
        self.memory.append(&ui::section("Memory", aside.as_ref(), &body));
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

fn texts(title: &str, note: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_hexpand(true);
    column.set_valign(gtk::Align::Center);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("row-label");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    column.append(&label);
    let note = ui::note(note);
    note.set_wrap(true);
    column.append(&note);
    column
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

/// "Today", or the day it was added: "16 Oct".
fn added(date: Option<&str>, today: Option<&glib::DateTime>) -> String {
    let Some(date) = date else { return String::new() };
    let parts: Vec<i32> = date.get(..10).unwrap_or(date).split('-').filter_map(|part| part.parse().ok()).collect();
    let [year, month, day] = parts[..] else { return date.to_owned() };
    let Ok(when) = glib::DateTime::from_local(year, month, day, 0, 0, 0.0) else { return date.to_owned() };
    if today.is_some_and(|today| today.year() == year && today.day_of_year() == when.day_of_year()) {
        return "Today".to_owned();
    }
    let format = if today.is_some_and(|today| today.year() == year) { "%-d %b" } else { "%-d %b %Y" };
    when.format(format).map(String::from).unwrap_or_else(|_| date.to_owned())
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
        "rank" => "Ranking",
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
            "rank" => {
                let items = line["input"]["items"].as_array().cloned().unwrap_or_default();
                let scores: Vec<f64> = answer["scores"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
                let best = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1));
                match best {
                    Some((at, value)) => (format!("Top: {}", items.get(at).and_then(Value::as_str).unwrap_or_default()), format!("{value:.2}")),
                    None => ("No items".to_owned(), "n/a".to_owned()),
                }
            }
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
        let rank = json!({ "time": 8, "call": "rank", "engine": "ollama", "input": { "query": "q", "items": ["Night light", "Sound"] }, "answer": { "scores": [0.9, 0.1] }, "error": null });
        assert_eq!((call(&rank).answer, call(&rank).score), ("Top: Night light".to_owned(), "0.90".to_owned()));
    }
}
