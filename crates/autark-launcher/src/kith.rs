//! Ask Kith: what you typed, sent to `autark-ai`'s `Route`, comes back as the actions it asks for, each
//! ticked if Kith is sure and run only when you say, and the answer to what it asks, streamed in with the
//! files it read. Built from the design's `AiLauncher` artboard.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use futures_util::StreamExt;
use gtk::prelude::*;
use gtk::{gio, glib};
use serde::Deserialize;
use serde_json::Value;

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
trait Ai {
    fn route(&self, text: &str) -> zbus::Result<String>;
    fn ask_files(&self, prompt: &str, files: &[&str]) -> zbus::Result<(u32, Vec<(String, String)>)>;
    fn stop(&self, id: u32) -> zbus::Result<()>;
    fn act(&self, actions: &str) -> zbus::Result<Vec<String>>;
    #[zbus(signal)]
    fn answer_piece(&self, id: u32, text: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn answer_done(&self, id: u32, error: String) -> zbus::Result<()>;
}

/// An action below this confidence is shown unticked, for you to tick if it's what you meant.
const SURE: f64 = 0.6;

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
    files: Vec<String>,
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
    chips: gtk::Box,
    stop: gtk::Button,
    open: gtk::Button,
    privacy: gtk::Label,
    jev: gtk::Switch,
    jev_box: gtk::Box,
    rows: RefCell<Vec<Row>>,
    ran: Cell<bool>,
    answering: Cell<Option<u32>>,
    opened: RefCell<Option<PathBuf>>,
    /// Counts each question, so an answer to one you've moved on from is dropped.
    generation: Cell<u64>,
    settings: Cell<Settings>,
    connection: RefCell<Option<zbus::Connection>>,
    close: Box<dyn Fn()>,
}

impl View {
    pub fn new(close: impl Fn() + 'static) -> Rc<Self> {
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
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let stop = gtk::Button::builder().label("Stop").focus_on_click(false).build();
        let open = gtk::Button::builder().focus_on_click(false).build();
        for button in [&stop, &open] {
            button.add_css_class("kith-button");
        }
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        chips.set_hexpand(true);
        bottom.append(&chips);
        bottom.append(&stop);
        bottom.append(&open);
        let answer = section("ANSWER", &answer_pill);
        answer.append(&text);
        answer.append(&bottom);

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
            chips,
            stop,
            open,
            privacy,
            jev,
            jev_box,
            rows: RefCell::default(),
            ran: Cell::new(false),
            answering: Cell::new(None),
            opened: RefCell::default(),
            generation: Cell::new(0),
            settings: Cell::default(),
            connection: RefCell::default(),
            close: Box::new(close),
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
        view.open.connect_clicked({
            let view = Rc::downgrade(&view);
            move |button| {
                let Some(view) = view.upgrade() else { return };
                let Some(path) = view.opened.borrow().clone() else { return };
                let context = WidgetExt::display(button).app_launch_context();
                if let Err(err) = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(&path).uri(), Some(&context)) {
                    eprintln!("launcher: opening {}: {err}", path.display());
                }
                (view.close)();
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
            let routed = async {
                let proxy = AiProxy::new(&view.connection().await?).await?;
                proxy.route(&text).await
            };
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
                        view.ask(generation, routed.question, routed.files).await;
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

    async fn connection(&self) -> zbus::Result<zbus::Connection> {
        if let Some(connection) = self.connection.borrow().clone() {
            return Ok(connection);
        }
        let connection = zbus::Connection::session().await?;
        *self.connection.borrow_mut() = Some(connection.clone());
        Ok(connection)
    }

    fn show_actions(self: &Rc<Self>, routed: &Routed, seconds: f64) {
        if routed.actions.is_empty() {
            return;
        }
        let engine = if routed.engine == "jev" { "Jev · cloud" } else { "Ollama · on this laptop" };
        self.route.set_label(&format!("{engine} · {seconds:.1} s"));
        for action in &routed.actions {
            let confidence = action["confidence"].as_f64().unwrap_or(0.0);
            let mut action = action.clone();
            if let Some(fields) = action.as_object_mut() {
                fields.remove("confidence");
            }
            let row = self.row(action, confidence);
            self.list.append(&row.row);
            self.rows.borrow_mut().push(row);
        }
        self.actions.set_visible(true);
        self.show_run();
    }

    fn row(self: &Rc<Self>, action: Value, confidence: f64) -> Row {
        let (icon, title, value, call) = describe(&action);
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
        let chosen: Vec<(usize, Value)> = self.rows.borrow().iter().enumerate().filter(|(_, row)| row.check.is_active()).map(|(at, row)| (at, row.action.clone())).collect();
        if chosen.is_empty() || self.ran.get() {
            return;
        }
        self.ran.set(true);
        self.run.set_sensitive(false);
        self.run.set_label("Running…");
        for row in self.rows.borrow().iter() {
            row.check.set_sensitive(false);
        }
        let actions = Value::Array(chosen.iter().map(|(_, action)| action.clone()).collect()).to_string();
        let generation = self.generation.get();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let results = async {
                let proxy = AiProxy::new(&view.connection().await?).await?;
                proxy.act(&actions).await
            };
            let results = results.await.map_err(said);
            if view.generation.get() != generation {
                return;
            }
            let rows = view.rows.borrow();
            for (position, (at, _)) in chosen.iter().enumerate() {
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

    async fn ask(self: &Rc<Self>, generation: u64, question: String, files: Vec<String>) {
        self.answer.set_visible(true);
        self.text.set_label("▍");
        self.stop.set_visible(true);
        self.open.set_visible(false);
        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let result = async {
            let proxy = AiProxy::new(&self.connection().await?).await?;
            // Listening before asking, so no piece is missed.
            let pieces = proxy.receive_answer_piece().await?;
            let dones = proxy.receive_answer_done().await?;
            let names: Vec<&str> = files.iter().map(String::as_str).collect();
            let (id, outcome) = proxy.ask_files(&question, &names).await?;
            zbus::Result::Ok((proxy, pieces, dones, id, outcome))
        };
        let (_proxy, pieces, dones, id, outcome) = match result.await {
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
        self.show_files(&outcome);
        enum Event {
            Piece(String),
            Done(String),
        }
        let pieces = pieces.filter_map(move |signal| std::future::ready(signal.args().ok().filter(|args| args.id == id).map(|args| Event::Piece(args.text))));
        let dones = dones.filter_map(move |signal| std::future::ready(signal.args().ok().filter(|args| args.id == id).map(|args| Event::Done(args.error))));
        let mut events = futures_util::stream::select(pieces, dones);
        let mut text = String::new();
        while let Some(event) = events.next().await {
            if self.generation.get() != generation {
                return;
            }
            match event {
                Event::Piece(piece) => {
                    text.push_str(&piece);
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

    fn show_files(&self, outcome: &[(String, String)]) {
        let mut first = None;
        for (path, why) in outcome {
            let chip = gtk::Label::builder()
                .label(if why.is_empty() { format!("read {path}") } else { format!("couldn’t read {path}: {why}") })
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .max_width_chars(36)
                .build();
            chip.set_tooltip_text(Some(&chip.label()));
            chip.add_css_class("kith-chip");
            if why.is_empty() {
                first.get_or_insert(path.clone());
            } else {
                chip.add_css_class("failed");
            }
            self.chips.append(&chip);
        }
        if let Some(path) = first {
            let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            let resolved = path.strip_prefix("~/").map_or_else(|| PathBuf::from(&path), |rest| home.join(rest));
            let name = resolved.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or(path);
            self.open.set_label(&format!("Open {name}"));
            *self.opened.borrow_mut() = Some(resolved);
        } else {
            *self.opened.borrow_mut() = None;
        }
    }

    fn end_answer(&self, error: &str) {
        self.answering.set(None);
        self.stop.set_visible(false);
        self.open.set_visible(self.opened.borrow().is_some());
        if !error.is_empty() {
            let text = self.text.label().trim_end_matches('▍').to_owned();
            self.text.set_label(&if text.is_empty() { error.to_owned() } else { format!("{text}\n\n{error}") });
        }
    }

    fn stop_answer(&self) {
        let Some(id) = self.answering.take() else { return };
        self.stop.set_visible(false);
        self.text.set_label(self.text.label().trim_end_matches('▍'));
        self.open.set_visible(self.opened.borrow().is_some());
        let connection = self.connection.borrow().clone();
        glib::spawn_future_local(async move {
            let Some(connection) = connection else { return };
            let stopped = async { AiProxy::new(&connection).await?.stop(id).await };
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

/// An action's icon, title, new value and what runs it, from `Act`'s catalog.
fn describe(action: &Value) -> (&'static str, String, String, String) {
    let on = action["on"].as_bool().unwrap_or(false);
    let percent = action["percent"].as_u64().unwrap_or(0);
    let state = if on { "on" } else { "off" }.to_owned();
    let (icon, title, value, call) = match action["action"].as_str().unwrap_or_default() {
        "set_volume" => ("audio-volume-high-symbolic", "Volume", format!("{percent}%"), format!("wpctl set-volume @DEFAULT_AUDIO_SINK@ {percent}%")),
        "mute" => (
            if on { "audio-volume-muted-symbolic" } else { "audio-volume-high-symbolic" },
            "Volume",
            if on { "muted" } else { "unmuted" }.to_owned(),
            format!("wpctl set-mute @DEFAULT_AUDIO_SINK@ {}", u8::from(on)),
        ),
        "set_brightness" => ("display-brightness-symbolic", "Display brightness", format!("{percent}%"), format!("login1.Session.SetBrightness(backlight, {percent}%)")),
        "wifi" => ("network-wireless-symbolic", "Wi-Fi", state, format!("iwd Device.Powered = {on}")),
        "focus" => ("notifications-disabled-symbolic", "Focus", state, format!("Notifications1.SetFocus({on})")),
        "night_light" => ("night-light-symbolic", "Night light", state, format!("NightLight1.SetActive({on})")),
        "lock" => ("system-lock-screen-symbolic", "Lock screen", String::new(), "login1.Session.Lock()".to_owned()),
        "sleep" => ("weather-clear-night-symbolic", "Sleep", String::new(), "login1.Manager.Suspend(true)".to_owned()),
        "open_app" => {
            let name = action["name"].as_str().unwrap_or_default().to_owned();
            ("application-x-executable-symbolic", "Open", name.clone(), format!("open the app called “{name}”"))
        }
        other => ("dialog-question-symbolic", other, String::new(), String::new()),
    };
    (icon, title.to_owned(), value, call)
}

/// A D-Bus error in words: autark-ai's own errors as it wrote them.
fn said(error: zbus::Error) -> String {
    match &error {
        zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" || name.as_str().starts_with("org.freedesktop.DBus.Error.Spawn.") => {
            "Kith isn’t running: start autark-ai.".to_owned()
        }
        zbus::Error::FDO(inner) if matches!(**inner, zbus::fdo::Error::ServiceUnknown(_)) => "Kith isn’t running: start autark-ai.".to_owned(),
        zbus::Error::MethodError(_, Some(message), _) => message.clone(),
        _ => error.to_string(),
    }
}
