//! Ask Kith: what you typed, routed by `autark-ai`, comes back as the laptop actions it asks for, each ticked if
//! Kith is sure and run only when you say, and a single answer to what it asks, streamed in. There is no
//! conversation: each question stands alone. Built from the design's `AiLauncher` artboard.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use futures_util::StreamExt;
use gtk::prelude::*;
use gtk::glib;
use serde::Deserialize;
use serde_json::Value;

use crate::ai::{self, describe, said};

/// An action below this confidence is shown unticked, for you to tick if it's what you meant.
pub const SURE: f64 = 0.6;

/// What the launcher needs from `ai.toml`; the rest is `autark-ai`'s.
#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default)]
pub struct Settings {
    enabled: bool,
    local_only: bool,
    features: Features,
    jev: Jev,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(default)]
struct Features {
    route: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self { route: true }
    }
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default)]
struct Jev {
    launcher: bool,
}

impl Settings {
    pub fn load() -> Self {
        autark_common::config::load("ai")
    }

    /// Whether results end with the Ask Kith row.
    pub fn offered(self) -> bool {
        self.enabled && self.features.route
    }
}

/// Turns Jev on or off for the launcher in the user's `ai.toml`, keeping its comments; the first change copies
/// the system's file across, as Settings does.
fn set_jev(on: bool) -> Result<(), String> {
    let paths = autark_common::config::paths("ai");
    let text = paths.iter().find_map(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    let mut document: toml_edit::DocumentMut = text.parse().map_err(|err| format!("ai.toml: {err}"))?;
    let jev = document.entry("jev").or_insert_with(toml_edit::table);
    let Some(jev) = jev.as_table_like_mut() else {
        return Err("ai.toml: jev isn't a table".to_owned());
    };
    jev.insert("launcher", toml_edit::value(on));
    let path = &paths[0];
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    std::fs::write(path, document.to_string()).map_err(|err| format!("{}: {err}", path.display()))
}

#[derive(Deserialize)]
struct Routed {
    actions: Vec<Value>,
    question: String,
    engine: String,
}

struct Row {
    /// The action as `Act` takes it.
    action: Value,
    check: gtk::CheckButton,
    done: gtk::Label,
    row: gtk::Box,
}

pub struct View {
    pub root: gtk::Box,
    pub footer: gtk::Box,
    status: gtk::Label,
    actions: gtk::Box,
    list: gtk::Box,
    route: gtk::Label,
    run: gtk::Button,
    answer: gtk::Box,
    text: gtk::Label,
    stop: gtk::Button,
    privacy: gtk::Label,
    jev: gtk::Switch,
    jev_box: gtk::Box,
    rows: RefCell<Vec<Row>>,
    ran: Cell<bool>,
    answering: Cell<Option<u32>>,
    /// Counts each question, so an answer to one you've moved on from is dropped.
    generation: Cell<u64>,
    settings: Cell<Settings>,
}

impl View {
    pub fn new() -> Rc<Self> {
        // Wrapped text asks for little width, so the dialog keeps its own instead of growing to fit a line.
        let status = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(40).build();
        status.add_css_class("kith-status");

        let route = pill();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let run = gtk::Button::builder().focus_on_click(false).halign(gtk::Align::End).build();
        run.add_css_class("kith-run");
        let actions = section("ACTIONS", &route);
        actions.append(&list);
        actions.append(&run);

        let answer_pill = pill();
        answer_pill.set_label("Ollama · on this laptop");
        let text = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(40).build();
        text.add_css_class("kith-answer");
        let stop = gtk::Button::builder().label("Stop").focus_on_click(false).halign(gtk::Align::End).build();
        stop.add_css_class("kith-button");
        let answer = section("ANSWER", &answer_pill);
        answer.append(&text);
        answer.append(&stop);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("kith");
        root.append(&status);
        root.append(&actions);
        root.append(&answer);

        let shield = gtk::Image::from_icon_name("security-high-symbolic");
        shield.set_pixel_size(14);
        let privacy = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).max_width_chars(30).build();
        let jev = gtk::Switch::builder().focus_on_click(false).valign(gtk::Align::Center).build();
        let jev_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        jev_box.append(&gtk::Label::new(Some("Use Jev for the launcher")));
        jev_box.append(&jev);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.add_css_class("footer");
        footer.add_css_class("kith-footer");
        footer.append(&shield);
        footer.append(&privacy);
        footer.append(&jev_box);

        let view = Rc::new(Self {
            root,
            footer,
            status,
            actions,
            list,
            route,
            run,
            answer,
            text,
            stop,
            privacy,
            jev,
            jev_box,
            rows: RefCell::default(),
            ran: Cell::new(false),
            answering: Cell::new(None),
            generation: Cell::new(0),
            settings: Cell::default(),
        });

        view.run.connect_clicked({
            let view = Rc::downgrade(&view);
            move |_| {
                if let Some(view) = view.upgrade() {
                    view.run();
                }
            }
        });
        view.stop.connect_clicked({
            let view = Rc::downgrade(&view);
            move |_| {
                if let Some(view) = view.upgrade() {
                    view.stop_answer();
                }
            }
        });
        view.jev.connect_state_set({
            let view = Rc::downgrade(&view);
            move |_, on| {
                let Some(view) = view.upgrade() else { return glib::Propagation::Proceed };
                let mut settings = view.settings.get();
                if settings.jev.launcher != on {
                    if let Err(err) = set_jev(on) {
                        eprintln!("launcher: {err}");
                        return glib::Propagation::Stop;
                    }
                    settings.jev.launcher = on;
                    view.settings.set(settings);
                    view.show_privacy();
                }
                glib::Propagation::Proceed
            }
        });
        view
    }

    /// Asks Kith about `text`, dropping whatever was asked before.
    pub fn start(self: &Rc<Self>, text: &str, settings: Settings) {
        self.cancel();
        let generation = self.generation.get();
        self.settings.set(settings);
        self.jev.set_active(settings.jev.launcher);
        self.jev_box.set_visible(!settings.local_only);
        self.show_privacy();
        self.rows.borrow_mut().clear();
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.ran.set(false);
        self.actions.set_visible(false);
        self.answer.set_visible(false);
        self.say("Kith is working out what you asked…");
        let text = text.trim().to_owned();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let start = Instant::now();
            let routed = async { ai::proxy().await?.route(&text).await };
            let routed = match routed.await {
                Ok(json) => serde_json::from_str::<Routed>(&json).map_err(|err| format!("Kith's answer doesn't fit: {err}")),
                Err(error) => Err(said(error)),
            };
            if view.generation.get() != generation {
                return;
            }
            match routed {
                Ok(routed) => {
                    view.status.set_visible(false);
                    view.show_actions(&routed, start.elapsed().as_secs_f64());
                    if !routed.question.is_empty() {
                        view.ask(generation, routed.question).await;
                    }
                }
                Err(error) => view.say(&error),
            }
        });
    }

    /// Drops the question under way, stopping its answer.
    pub fn cancel(&self) {
        self.generation.set(self.generation.get() + 1);
        self.stop_answer();
    }

    /// The Enter key: runs the ticked actions, once.
    pub fn activate(self: &Rc<Self>) {
        if self.run.is_visible() && self.run.is_sensitive() {
            self.run();
        }
    }

    fn say(&self, text: &str) {
        self.status.set_label(text);
        self.status.set_visible(true);
    }

    fn show_privacy(&self) {
        let settings = self.settings.get();
        let mut text = "Local only: nothing left this laptop.".to_owned();
        if settings.jev.launcher && !settings.local_only {
            text.push_str(" Ollama answers until Jev’s client is built.");
        }
        self.privacy.set_label(&text);
    }

    fn show_actions(self: &Rc<Self>, routed: &Routed, seconds: f64) {
        if routed.actions.is_empty() {
            return;
        }
        let engine = if routed.engine == "jev" { "Jev · cloud" } else { "Ollama · on this laptop" };
        self.route.set_label(&format!("{engine} · {seconds:.1} s"));
        for action in &routed.actions {
            let row = self.row(action, action["confidence"].as_f64().unwrap_or(0.0));
            self.list.append(&row.row);
            self.rows.borrow_mut().push(row);
        }
        self.actions.set_visible(true);
        self.show_run();
    }

    fn row(self: &Rc<Self>, action: &Value, confidence: f64) -> Row {
        let (icon, title, value, call) = describe(action);
        let check = gtk::CheckButton::builder().active(confidence >= SURE).focus_on_click(false).valign(gtk::Align::Center).build();
        check.connect_toggled({
            let view = Rc::downgrade(self);
            move |_| {
                if let Some(view) = view.upgrade() {
                    view.show_run();
                }
            }
        });
        let image = gtk::Image::from_icon_name(icon);
        image.set_pixel_size(16);
        let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tile.add_css_class("kith-tile");
        tile.set_valign(gtk::Align::Center);
        tile.set_homogeneous(true);
        tile.append(&image);
        let label = if value.is_empty() { title } else { format!("{title} → {value}") };
        let title = gtk::Label::builder().label(label).xalign(0.0).build();
        title.add_css_class("title");
        let call = gtk::Label::builder().label(call).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        call.add_css_class("kith-call");
        let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
        words.set_hexpand(true);
        words.set_valign(gtk::Align::Center);
        words.append(&title);
        words.append(&call);
        let sure = gtk::Label::new(Some(&format!("confidence {confidence:.2}")));
        sure.add_css_class("kith-confidence");
        let bar = gtk::ProgressBar::builder().fraction(confidence.clamp(0.0, 1.0)).valign(gtk::Align::Center).build();
        bar.add_css_class("kith-bar");
        let done = gtk::Label::builder().wrap(true).max_width_chars(24).xalign(1.0).build();
        done.add_css_class("kith-done");
        done.set_visible(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("kith-action");
        for widget in [check.upcast_ref::<gtk::Widget>(), tile.upcast_ref(), words.upcast_ref(), sure.upcast_ref(), bar.upcast_ref(), done.upcast_ref()] {
            row.append(widget);
        }
        let mut action = action.clone();
        if let Some(fields) = action.as_object_mut() {
            fields.remove("confidence");
        }
        Row { action, check, done, row }
    }

    fn show_run(&self) {
        let rows = self.rows.borrow();
        let count = rows.iter().filter(|row| row.check.is_active()).count();
        let noun = if count == 1 { "action" } else { "actions" };
        if !self.ran.get() {
            self.run.set_label(&format!("Run {count} {noun} ↵"));
            self.run.set_sensitive(count > 0);
        }
    }

    fn run(self: &Rc<Self>) {
        let chosen: Vec<usize> = self.rows.borrow().iter().enumerate().filter(|(_, row)| row.check.is_active()).map(|(at, _)| at).collect();
        if chosen.is_empty() || self.ran.get() {
            return;
        }
        self.ran.set(true);
        self.run.set_sensitive(false);
        self.run.set_label("Running…");
        for row in self.rows.borrow().iter() {
            row.check.set_sensitive(false);
        }
        let actions = Value::Array(chosen.iter().map(|at| self.rows.borrow()[*at].action.clone()).collect()).to_string();
        let generation = self.generation.get();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let results = async { ai::proxy().await?.act(&actions).await };
            let results = results.await.map_err(said);
            if view.generation.get() != generation {
                return;
            }
            let rows = view.rows.borrow();
            for (position, at) in chosen.iter().enumerate() {
                let row = &rows[*at];
                let outcome = match &results {
                    Ok(results) => results.get(position).cloned().unwrap_or_else(|| "no answer from Kith".to_owned()),
                    Err(error) => error.clone(),
                };
                if outcome.is_empty() {
                    row.done.set_label("Done");
                    row.row.add_css_class("done");
                } else {
                    row.done.set_label(&outcome);
                    row.done.add_css_class("failed");
                }
                row.done.set_visible(true);
            }
            // "Ran" counts the ones that went through.
            let ran = rows.iter().filter(|row| row.row.has_css_class("done")).count();
            view.run.set_label(&format!("Ran {ran} {}", if ran == 1 { "action" } else { "actions" }));
        });
    }

    async fn ask(self: &Rc<Self>, generation: u64, question: String) {
        self.answer.set_visible(true);
        self.text.set_label("▍");
        self.stop.set_visible(true);
        let result = async {
            let proxy = ai::proxy().await?;
            // Listening before asking, so no piece is missed.
            let pieces = proxy.receive_answer_piece().await?;
            let dones = proxy.receive_answer_done().await?;
            let id = proxy.ask(&question, "").await?;
            zbus::Result::Ok((proxy, pieces, dones, id))
        };
        let (_proxy, pieces, dones, id) = match result.await {
            Ok(started) => started,
            Err(error) => {
                if self.generation.get() == generation {
                    self.end_answer(&said(error));
                }
                return;
            }
        };
        if self.generation.get() != generation {
            return;
        }
        self.answering.set(Some(id));
        enum Event {
            Piece(u32, String),
            Done(String),
        }
        let pieces = pieces.filter_map(move |signal| std::future::ready(signal.args().ok().filter(|args| args.id == id).map(|args| Event::Piece(args.offset, args.text))));
        let dones = dones.filter_map(move |signal| std::future::ready(signal.args().ok().filter(|args| args.id == id).map(|args| Event::Done(args.error))));
        let mut events = futures_util::stream::select(pieces, dones);
        let mut text = String::new();
        while let Some(event) = events.next().await {
            if self.generation.get() != generation {
                return;
            }
            match event {
                Event::Piece(offset, piece) => {
                    ai::merge(&mut text, offset, &piece);
                    self.text.set_label(&format!("{text}▍"));
                }
                Event::Done(error) => {
                    self.text.set_label(&text);
                    self.end_answer(&error);
                    return;
                }
            }
        }
    }

    fn end_answer(&self, error: &str) {
        self.answering.set(None);
        self.stop.set_visible(false);
        if !error.is_empty() {
            let text = self.text.label().trim_end_matches('▍').to_owned();
            self.text.set_label(&if text.is_empty() { error.to_owned() } else { format!("{text}\n\n{error}") });
        }
    }

    fn stop_answer(&self) {
        let Some(id) = self.answering.take() else { return };
        self.stop.set_visible(false);
        self.text.set_label(self.text.label().trim_end_matches('▍'));
        glib::spawn_future_local(async move {
            let stopped = async { ai::proxy().await?.stop(id).await };
            if let Err(err) = stopped.await {
                eprintln!("launcher: stopping Kith's answer: {err}");
            }
        });
    }
}

fn pill() -> gtk::Label {
    let pill = gtk::Label::new(Some("Ollama · on this laptop · a few seconds"));
    pill.add_css_class("kith-pill");
    pill.set_halign(gtk::Align::End);
    pill.set_hexpand(true);
    pill
}

fn section(heading: &str, pill: &gtk::Label) -> gtk::Box {
    let label = gtk::Label::builder().label(heading).xalign(0.0).build();
    label.add_css_class("group");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&label);
    top.append(pill);
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class("kith-section");
    section.append(&top);
    section
}
