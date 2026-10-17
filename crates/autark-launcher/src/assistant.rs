//! The Assistant panel: Kith's conversation, docked at the top right and left open beside your windows, built
//! from the design's `Assistant` artboard. It shows the one running thread `autark-ai` keeps, Ask Kith's
//! exchanges included, with each message's actions to run, its answer as it's written and the facts Kith
//! proposes to remember, each waiting for Save, Edit or Don't save. Opened by Follow up in Ask Kith or by
//! `autark-launcher --assistant`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use serde_json::Value;

use crate::ai::{self, describe, said};
use crate::face::Face;

/// An answer still being written, so its pieces can be added as they arrive.
struct Writing {
    label: gtk::Label,
    text: RefCell<String>,
}

pub struct Panel {
    window: gtk::ApplicationWindow,
    list: gtk::Box,
    scroller: gtk::ScrolledWindow,
    entry: gtk::Entry,
    memory: gtk::Button,
    status: gtk::Label,
    /// Each entry shown, by id: its JSON, to tell when it changed, and its widget.
    shown: RefCell<Vec<(u64, String, gtk::Widget)>>,
    writing: RefCell<HashMap<u32, Rc<Writing>>>,
    /// Shown while a message is being routed.
    thinking: gtk::Label,
    sending: Cell<bool>,
    listening: Cell<bool>,
    _face: Rc<Face>,
}

impl Panel {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::new(app);
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("autark-assistant"));
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Right, true);
        // Beside your windows rather than over them: the keyboard only when you click into it.
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.add_css_class("assistant-window");

        let face = Face::new(34);
        let title = gtk::Label::new(Some("Kith"));
        title.add_css_class("assistant-title");
        let pill = gtk::Label::new(Some("Ollama · on this laptop"));
        pill.add_css_class("kith-pill");
        pill.set_valign(gtk::Align::Center);
        let memory = gtk::Button::builder().focus_on_click(false).valign(gtk::Align::Center).tooltip_text("Open what Kith remembers in Settings").build();
        memory.add_css_class("assistant-memory");
        let clear = gtk::Button::builder().icon_name("user-trash-symbolic").focus_on_click(false).valign(gtk::Align::Center).tooltip_text("Clear the conversation").build();
        clear.add_css_class("assistant-icon");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.add_css_class("assistant-header");
        for widget in [face.visor.widget().upcast_ref::<gtk::Widget>(), title.upcast_ref(), pill.upcast_ref(), spacer.upcast_ref(), memory.upcast_ref(), clear.upcast_ref()] {
            header.append(widget);
        }

        let list = gtk::Box::new(gtk::Orientation::Vertical, 14);
        list.add_css_class("assistant-list");
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&list).build();

        let thinking = gtk::Label::builder().label("Kith is thinking…").xalign(0.0).halign(gtk::Align::Start).build();
        thinking.add_css_class("assistant-thinking");

        let status = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(30).visible(false).build();
        status.add_css_class("assistant-status");
        let entry = gtk::Entry::builder().placeholder_text("Message Kith").hexpand(true).build();
        face.listen_to(&entry);
        let send = gtk::Button::builder().icon_name("go-up-symbolic").focus_on_click(false).tooltip_text("Send").valign(gtk::Align::Center).build();
        send.add_css_class("assistant-send");
        let input = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        input.add_css_class("assistant-input");
        input.append(&entry);
        input.append(&send);
        let note = gtk::Label::new(Some("History kept 7 days on this laptop · memory only with your approval"));
        note.add_css_class("assistant-note");
        let bottom = gtk::Box::new(gtk::Orientation::Vertical, 8);
        bottom.add_css_class("assistant-bottom");
        bottom.append(&status);
        bottom.append(&input);
        bottom.append(&note);

        let panel_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel_box.add_css_class("assistant");
        // The design's 460 by 736, plus the margins that hold the shadow.
        panel_box.set_size_request(460 + 56, 736 + 72);
        panel_box.append(&header);
        panel_box.append(&scroller);
        panel_box.append(&bottom);
        window.set_child(Some(&panel_box));

        let panel = Rc::new(Self {
            window,
            list,
            scroller,
            entry,
            memory,
            status,
            shown: RefCell::default(),
            writing: RefCell::default(),
            thinking,
            sending: Cell::new(false),
            listening: Cell::new(false),
            _face: face,
        });

        let send_message = {
            let panel = Rc::downgrade(&panel);
            move || {
                if let Some(panel) = panel.upgrade() {
                    panel.send();
                }
            }
        };
        panel.entry.connect_activate({
            let send_message = send_message.clone();
            move |_| send_message()
        });
        send.connect_clicked(move |_| send_message());
        panel.memory.connect_clicked(|_| open_settings());
        clear.connect_clicked(|_| {
            glib::spawn_future_local(async {
                if let Err(err) = async { ai::proxy().await?.clear_history().await }.await {
                    eprintln!("launcher: clearing Kith's conversation: {err}");
                }
            });
        });
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let panel = Rc::downgrade(&panel);
            move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape
                    && let Some(panel) = panel.upgrade()
                {
                    panel.window.set_visible(false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        panel.window.add_controller(keys);
        panel
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.window.is_visible() { self.window.set_visible(false) } else { self.open() }
    }

    /// Shows the panel with the conversation as it is, ready to type into.
    pub fn open(self: &Rc<Self>) {
        self.listen();
        self.reload();
        self.window.present();
        self.entry.grab_focus();
    }

    /// Follows the conversation while the launcher runs: changes, and answers as they're written.
    fn listen(self: &Rc<Self>) {
        if self.listening.replace(true) {
            return;
        }
        let panel = self.clone();
        glib::spawn_future_local(async move {
            let result = async {
                let proxy = ai::proxy().await?;
                let changes = proxy.receive_history_changed().await?.map(|_| Event::Changed);
                let pieces = proxy.receive_answer_piece().await?.filter_map(|signal| std::future::ready(signal.args().ok().map(|args| Event::Piece(args.id, args.offset, args.text))));
                let mut events = futures_util::stream::select(changes, pieces);
                while let Some(event) = events.next().await {
                    match event {
                        Event::Changed => panel.reload(),
                        Event::Piece(id, offset, piece) => panel.piece(id, offset, &piece),
                    }
                }
                zbus::Result::Ok(())
            };
            if let Err(err) = result.await {
                eprintln!("launcher: following Kith's conversation: {err}");
            }
            panel.listening.set(false);
        });
    }

    fn piece(&self, id: u32, offset: u32, piece: &str) {
        let Some(writing) = self.writing.borrow().get(&id).cloned() else { return };
        ai::merge(&mut writing.text.borrow_mut(), offset, piece);
        writing.label.set_label(&format!("{}▍", writing.text.borrow()));
        self.follow_bottom();
    }

    /// Reads the conversation again and shows what changed, keeping the widgets of entries that didn't.
    fn reload(self: &Rc<Self>) {
        self.memory.set_label(&format!("Memory · {}", memory_count()));
        let panel = self.clone();
        glib::spawn_future_local(async move {
            let history = async { ai::proxy().await?.history().await }.await;
            let entries: Vec<Value> = match history {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(error) => {
                    panel.say(&said(error));
                    return;
                }
            };
            panel.show(&entries);
        });
    }

    fn show(self: &Rc<Self>, entries: &[Value]) {
        let old = std::mem::take(&mut *self.shown.borrow_mut());
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut shown = Vec::new();
        let mut writing = HashMap::new();
        for entry in entries {
            let id = entry["id"].as_u64().unwrap_or(0);
            let json = entry.to_string();
            let kept = old.iter().find(|(old_id, old_json, _)| *old_id == id && *old_json == json).map(|(.., widget)| widget.clone());
            let was_kept = kept.is_some();
            let widget = kept.unwrap_or_else(|| self.entry_widget(entry));
            if entry["kind"] == "answer" && entry["writing"].as_bool() == Some(true) {
                let answer = entry["answer"].as_u64().unwrap_or(0) as u32;
                let known = self.writing.borrow().get(&answer).cloned();
                // A kept widget keeps the pieces it has; a new one starts from the text read.
                let state = match known {
                    Some(state) if was_kept => state,
                    _ => {
                        let label = find_label(&widget, "assistant-answer");
                        let text = entry["text"].as_str().unwrap_or_default().to_owned();
                        Rc::new(Writing { label, text: RefCell::new(text) })
                    }
                };
                writing.insert(answer, state);
            }
            self.list.append(&widget);
            shown.push((id, json, widget));
        }
        if self.sending.get() {
            self.list.append(&self.thinking);
        }
        if entries.is_empty() && !self.sending.get() {
            let empty = gtk::Label::new(Some("Ask Kith anything, or tell it to do something."));
            empty.add_css_class("assistant-empty");
            self.list.append(&empty);
        }
        *self.shown.borrow_mut() = shown;
        *self.writing.borrow_mut() = writing;
        self.follow_bottom();
    }

    fn entry_widget(self: &Rc<Self>, entry: &Value) -> gtk::Widget {
        let id = entry["id"].as_u64().unwrap_or(0);
        match entry["kind"].as_str().unwrap_or_default() {
            "you" => {
                let label = bubble_label(entry["text"].as_str().unwrap_or_default());
                label.add_css_class("assistant-you");
                label.set_halign(gtk::Align::End);
                label.upcast()
            }
            "actions" => self.actions_card(id, entry),
            "answer" => answer_card(entry),
            "memory" => memory_card(id, entry["text"].as_str().unwrap_or_default(), entry["state"].as_str().unwrap_or_default()).upcast(),
            "task" => task_card(id, &entry["task"], entry["state"].as_str().unwrap_or_default(), entry["unit"].as_str().unwrap_or_default(), entry["next"].as_u64().unwrap_or(0)).upcast(),
            "suggestion" => suggestion_card(entry).upcast(),
            _ => gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
        }
    }

    fn actions_card(self: &Rc<Self>, id: u64, entry: &Value) -> gtk::Widget {
        let actions = entry["actions"].as_array().cloned().unwrap_or_default();
        let results = entry["results"].as_array().cloned().unwrap_or_default();
        let skipped = entry["skipped"].as_bool() == Some(true);
        let card = card("assistant-actions");
        let caption = gtk::Label::builder().xalign(0.0).build();
        caption.add_css_class("assistant-caption");
        card.append(&caption);
        if !results.is_empty() {
            let ran = results.iter().filter(|result| result.as_str() == Some("")).count();
            caption.set_label(&format!("Ran {ran} {}", if ran == 1 { "action" } else { "actions" }));
            for (action, result) in actions.iter().zip(&results) {
                let (_, title, value, _) = describe(action);
                let what = if value.is_empty() { title } else { format!("{title} → {value}") };
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                line.add_css_class("assistant-ran");
                let (mark, text, class) = match result.as_str() {
                    Some("") => ("✓", what, "done"),
                    Some(why) => ("✗", format!("{what} · {why}"), "failed"),
                    None => ("–", what, "skipped"),
                };
                let mark = gtk::Label::new(Some(mark));
                mark.add_css_class("mark");
                let text = gtk::Label::builder().label(text).xalign(0.0).wrap(true).max_width_chars(30).build();
                line.add_css_class(class);
                line.append(&mark);
                line.append(&text);
                card.append(&line);
            }
            return card.upcast();
        }
        caption.set_label(if skipped { "Not run" } else { "Run these?" });
        let confidence = entry["confidence"].as_array().cloned().unwrap_or_default();
        let mut checks = Vec::new();
        for (at, action) in actions.iter().enumerate() {
            let (_, title, value, _) = describe(action);
            let what = if value.is_empty() { title } else { format!("{title} → {value}") };
            let sure = confidence.get(at).and_then(Value::as_f64).unwrap_or(1.0) >= crate::kith::SURE;
            let check = gtk::CheckButton::builder().label(what).active(!skipped && sure).sensitive(!skipped).focus_on_click(false).build();
            card.append(&check);
            checks.push(check);
        }
        if !skipped {
            let run = gtk::Button::builder().focus_on_click(false).halign(gtk::Align::Start).build();
            run.add_css_class("kith-run");
            let label = {
                let (checks, run) = (checks.clone(), run.clone());
                move || {
                    let count = checks.iter().filter(|check| check.is_active()).count();
                    run.set_label(&format!("Run {count} {}", if count == 1 { "action" } else { "actions" }));
                    run.set_sensitive(count > 0);
                }
            };
            label();
            for check in &checks {
                let label = label.clone();
                check.connect_toggled(move |_| label());
            }
            run.connect_clicked({
                let panel = Rc::downgrade(self);
                move |run| {
                    let chosen: Vec<u32> = checks.iter().enumerate().filter(|(_, check)| check.is_active()).map(|(at, _)| at as u32).collect();
                    run.set_sensitive(false);
                    run.set_label("Running…");
                    let panel = panel.clone();
                    glib::spawn_future_local(async move {
                        // The card is redrawn from the conversation once they've run.
                        if let Err(error) = async { ai::proxy().await?.chat_act(id, &chosen).await }.await
                            && let Some(panel) = panel.upgrade()
                        {
                            panel.say(&said(error));
                            panel.reload();
                        }
                    });
                }
            });
            card.append(&run);
        }
        card.upcast()
    }

    fn send(self: &Rc<Self>) {
        let text = self.entry.text().trim().to_owned();
        if text.is_empty() || self.sending.get() {
            return;
        }
        self.entry.set_text("");
        self.status.set_visible(false);
        self.sending.set(true);
        self.list.append(&self.thinking);
        self.follow_bottom();
        let panel = self.clone();
        glib::spawn_future_local(async move {
            let result = async {
                let proxy = ai::proxy().await?;
                let routed: Value = serde_json::from_str(&proxy.chat(&text).await?).unwrap_or_default();
                let question = routed["question"].as_str().unwrap_or_default();
                if !question.is_empty() {
                    let files: Vec<&str> = routed["files"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    proxy.chat_answer(question, &files).await?;
                }
                zbus::Result::Ok(())
            };
            let result = result.await;
            panel.sending.set(false);
            if panel.thinking.parent().is_some() {
                panel.list.remove(&panel.thinking);
            }
            if let Err(error) = result {
                panel.say(&said(error));
            }
            panel.reload();
        });
    }

    fn say(&self, text: &str) {
        self.status.set_label(text);
        self.status.set_visible(true);
    }

    /// Keeps the newest in view.
    fn follow_bottom(&self) {
        let adjustment = self.scroller.vadjustment();
        glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper() - adjustment.page_size()));
    }
}

enum Event {
    Changed,
    Piece(u32, u32, String),
}

fn bubble_label(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(34).selectable(true).build();
    label.set_focusable(false);
    label
}

fn card(class: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("assistant-card");
    card.add_css_class(class);
    card.set_halign(gtk::Align::Fill);
    card
}

fn answer_card(entry: &Value) -> gtk::Widget {
    let card = card("assistant-kith");
    let writing = entry["writing"].as_bool() == Some(true);
    let text = entry["text"].as_str().unwrap_or_default();
    let label = bubble_label(&if writing { format!("{text}▍") } else { text.to_owned() });
    label.add_css_class("assistant-answer");
    card.append(&label);
    let error = entry["error"].as_str().unwrap_or_default();
    if !error.is_empty() {
        let why = gtk::Label::builder().label(error).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(34).build();
        why.add_css_class("assistant-error");
        card.append(&why);
    }
    let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    chips.set_halign(gtk::Align::Start);
    for file in entry["files"].as_array().into_iter().flatten() {
        let (path, why) = (file[0].as_str().unwrap_or_default(), file[1].as_str().unwrap_or_default());
        let chip = gtk::Label::builder()
            .label(if why.is_empty() { format!("read {path}") } else { format!("couldn’t read {path}") })
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(30)
            .tooltip_text(if why.is_empty() { path.to_owned() } else { format!("{path}: {why}") })
            .build();
        chip.add_css_class("kith-chip");
        if !why.is_empty() {
            chip.add_css_class("failed");
        }
        chips.append(&chip);
    }
    if chips.first_child().is_some() {
        card.append(&chips);
    }
    if writing {
        let id = entry["answer"].as_u64().unwrap_or(0) as u32;
        let stop = gtk::Button::builder().label("Stop").focus_on_click(false).halign(gtk::Align::Start).build();
        stop.add_css_class("kith-button");
        stop.connect_clicked(move |stop| {
            stop.set_sensitive(false);
            glib::spawn_future_local(async move {
                if let Err(err) = async { ai::proxy().await?.stop(id).await }.await {
                    eprintln!("launcher: stopping Kith's answer: {err}");
                }
            });
        });
        card.append(&stop);
    }
    card.upcast()
}

/// A fact Kith proposes to remember, with Save, Edit and Don't save until you answer, then what you chose.
/// Shared with Ask Kith.
pub fn memory_card(entry: u64, text: &str, state: &str) -> gtk::Box {
    let card = card("assistant-memory-card");
    let caption = gtk::Label::builder().label("SAVE TO MEMORY?").xalign(0.0).build();
    caption.add_css_class("assistant-caption");
    let fact = gtk::Label::builder().label(text).xalign(0.0).wrap(true).max_width_chars(34).build();
    fact.add_css_class("assistant-fact");
    card.append(&caption);
    card.append(&fact);
    if state != "proposed" {
        caption.set_label("MEMORY");
        card.append(&outcome(state == "saved"));
        return card;
    }
    let edit_entry = gtk::Entry::builder().text(text).visible(false).build();
    card.append(&edit_entry);
    let save = gtk::Button::builder().label("Save").focus_on_click(false).build();
    save.add_css_class("assistant-save");
    let edit = gtk::Button::builder().label("Edit").focus_on_click(false).build();
    edit.add_css_class("assistant-edit");
    let skip = gtk::Button::builder().label("Don’t save").focus_on_click(false).build();
    skip.add_css_class("assistant-skip");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&save);
    buttons.append(&edit);
    buttons.append(&skip);
    card.append(&buttons);
    let why = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(34).visible(false).build();
    why.add_css_class("assistant-error");
    card.append(&why);

    edit.connect_clicked({
        let (fact, edit_entry) = (fact.clone(), edit_entry.clone());
        move |edit| {
            fact.set_visible(false);
            edit_entry.set_visible(true);
            edit_entry.grab_focus();
            edit.set_visible(false);
        }
    });
    let answer = {
        let (card, fact, edit_entry, buttons, why) = (card.clone(), fact.clone(), edit_entry.clone(), buttons.clone(), why.clone());
        move |save: bool| {
            let text = if WidgetExt::is_visible(&edit_entry) { edit_entry.text().trim().to_owned() } else { fact.label().to_string() };
            buttons.set_sensitive(false);
            let (card, fact, edit_entry, buttons, why) = (card.clone(), fact.clone(), edit_entry.clone(), buttons.clone(), why.clone());
            glib::spawn_future_local(async move {
                match async { ai::proxy().await?.remember(entry, &text, save).await }.await {
                    Ok(()) => {
                        fact.set_label(&text);
                        fact.set_visible(true);
                        edit_entry.set_visible(false);
                        buttons.set_visible(false);
                        why.set_visible(false);
                        card.append(&outcome(save));
                    }
                    Err(error) => {
                        why.set_label(&said(error));
                        why.set_visible(true);
                        buttons.set_sensitive(true);
                    }
                }
            });
        }
    };
    save.connect_clicked({
        let answer = answer.clone();
        move |_| answer(true)
    });
    edit_entry.connect_activate({
        let answer = answer.clone();
        move |_| answer(true)
    });
    skip.connect_clicked(move |_| answer(false));
    card
}

/// How long a suggestion stands, as `autark-ai` keeps it.
const SUGGESTION_STANDS: u64 = 60 * 60;

/// A suggestion Kith made from something it noticed, from the design's suggestion card: what happened, its one
/// offer, Not now and Don't suggest again until you answer, then what you chose.
fn suggestion_card(entry: &Value) -> gtk::Box {
    let card = card("assistant-suggestion-card");
    let id = entry["id"].as_u64().unwrap_or(0);
    let caption = gtk::Label::builder().label("SUGGESTION").xalign(0.0).build();
    caption.add_css_class("assistant-caption");
    let rated = gtk::Label::builder().label("Ollama rated this worth a quick look").xalign(0.0).build();
    rated.add_css_class("assistant-key");
    let text = bubble_label(entry["text"].as_str().unwrap_or_default());
    text.add_css_class("assistant-fact");
    card.append(&caption);
    card.append(&rated);
    card.append(&text);
    let offer_label = entry["offer_label"].as_str().unwrap_or_default().to_owned();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |now| now.as_secs());
    let passed = now.saturating_sub(entry["time"].as_u64().unwrap_or(0)) > SUGGESTION_STANDS;
    match entry["state"].as_str().unwrap_or_default() {
        "open" if !passed => {}
        "open" => {
            card.append(&pill("Passed by"));
            return card;
        }
        state => {
            card.append(&suggestion_outcome(state, &offer_label, entry["result"].as_str().unwrap_or_default()));
            return card;
        }
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let offer = gtk::Button::builder().label(&offer_label).focus_on_click(false).visible(!entry["offer"].is_null()).build();
    offer.add_css_class("assistant-save");
    let not_now = gtk::Button::builder().label("Not now").focus_on_click(false).build();
    not_now.add_css_class("assistant-edit");
    let never = gtk::Button::builder().label("Don’t suggest again").focus_on_click(false).build();
    never.add_css_class("assistant-skip");
    buttons.append(&offer);
    buttons.append(&not_now);
    buttons.append(&never);
    card.append(&buttons);
    let why = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(34).visible(false).build();
    why.add_css_class("assistant-error");
    card.append(&why);
    for (button, answer) in [(&offer, "accept"), (&not_now, "not_now"), (&never, "never")] {
        let (card, buttons, why, offer_label) = (card.clone(), buttons.clone(), why.clone(), offer_label.clone());
        button.connect_clicked(move |_| {
            buttons.set_sensitive(false);
            let (card, buttons, why, offer_label) = (card.clone(), buttons.clone(), why.clone(), offer_label.clone());
            glib::spawn_future_local(async move {
                match async { ai::proxy().await?.answer_suggestion(id, answer).await }.await {
                    Ok(result) => {
                        buttons.set_visible(false);
                        why.set_visible(false);
                        let state = match answer {
                            "accept" => "accepted",
                            other => other,
                        };
                        card.append(&suggestion_outcome(state, &offer_label, &result));
                    }
                    Err(error) => {
                        why.set_label(&said(error));
                        why.set_visible(true);
                        buttons.set_sensitive(true);
                    }
                }
            });
        });
    }
    card
}

/// What became of a suggestion: its offer done or why not, Not now, or never again.
fn suggestion_outcome(state: &str, offer: &str, result: &str) -> gtk::Widget {
    match state {
        "accepted" if result.is_empty() => pill(&format!("{offer} · done")).upcast(),
        "accepted" => {
            let why = gtk::Label::builder().label(format!("{offer} didn’t work: {result}")).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(34).build();
            why.add_css_class("assistant-error");
            why.upcast()
        }
        "never" => pill("Won’t suggest this again · undo in Settings › AI").upcast(),
        _ => pill("Not now").upcast(),
    }
}

fn pill(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(40).halign(gtk::Align::Start).build();
    label.add_css_class("assistant-outcome");
    label
}

/// The schedules a task can have, as `autark-ai` takes them and as the card says them.
const EVERY: [(&str, &str); 11] = [
    ("day", "Every day"),
    ("weekdays", "Weekdays"),
    ("weekends", "Weekends"),
    ("monday", "Mondays"),
    ("tuesday", "Tuesdays"),
    ("wednesday", "Wednesdays"),
    ("thursday", "Thursdays"),
    ("friday", "Fridays"),
    ("saturday", "Saturdays"),
    ("sunday", "Sundays"),
    ("month", "Monthly"),
];

/// A task's schedule in words: "Every day at 08:00", "The 1st of each month at 09:00".
fn when_words(when: &Value) -> String {
    let every = when["every"].as_str().unwrap_or_default();
    let time = when["time"].as_str().unwrap_or_default();
    if every == "month" {
        let day = when["day"].as_u64().unwrap_or(1);
        let suffix = match (day % 10, day % 100) {
            (_, 11..=13) => "th",
            (1, _) => "st",
            (2, _) => "nd",
            (3, _) => "rd",
            _ => "th",
        };
        return format!("The {day}{suffix} of each month at {time}");
    }
    let words = EVERY.iter().find(|(key, _)| *key == every).map_or(every, |(_, words)| words);
    format!("{words} at {time}")
}

/// When a timer first fires, from microseconds since the epoch: "today at 08:00", "tomorrow at 08:00".
fn first_run(next: u64) -> String {
    let Ok(when) = glib::DateTime::from_unix_local((next / 1_000_000) as i64) else { return String::new() };
    let Ok(now) = glib::DateTime::now_local() else { return String::new() };
    let days = |date: &glib::DateTime| date.year() * 400 + date.day_of_year();
    let time = when.format("%H:%M").map(|time| time.to_string()).unwrap_or_default();
    match days(&when) - days(&now) {
        0 => format!("today at {time}"),
        1 => format!("tomorrow at {time}"),
        _ => format!("{} at {time}", when.format("%a %-d %b").map(|date| date.to_string()).unwrap_or_default()),
    }
}

/// What a created task's card says: when it first runs, if systemd said, and its timer.
fn created(unit: &str, next: u64) -> String {
    if next == 0 { format!("Created · {unit}") } else { format!("Created · first run {} · {unit}", first_run(next)) }
}

/// How long a run may take, from ai.toml's `[tasks]`, as `autark-ai` reads it.
fn task_minutes() -> u64 {
    #[derive(serde::Deserialize, Default)]
    struct Ai {
        #[serde(default)]
        tasks: Tasks,
    }
    #[derive(serde::Deserialize)]
    struct Tasks {
        #[serde(default = "five")]
        minutes: u64,
    }
    fn five() -> u64 {
        5
    }
    impl Default for Tasks {
        fn default() -> Self {
            Self { minutes: five() }
        }
    }
    autark_common::config::load::<Ai>("ai").tasks.minutes
}

/// A task Kith proposes, with what it does, when, what it reads, where it runs and how it's delivered: Create
/// task, Edit (name, time and inputs) or Cancel, then what came of it. Shared with Ask Kith.
pub fn task_card(entry: u64, task: &Value, state: &str, unit: &str, next: u64) -> gtk::Box {
    let card = card("assistant-task-card");
    let caption = gtk::Label::builder().label("NEW TASK").xalign(0.0).build();
    caption.add_css_class("assistant-caption");
    card.append(&caption);
    let grid = gtk::Grid::builder().column_spacing(14).row_spacing(6).build();
    let mut row = 0;
    let mut add = |name: &str, value: &gtk::Widget| {
        let label = gtk::Label::builder().label(name).xalign(0.0).yalign(0.0).build();
        label.add_css_class("assistant-key");
        grid.attach(&label, 0, row, 1, 1);
        value.set_hexpand(true);
        grid.attach(value, 1, row, 1, 1);
        row += 1;
    };
    let text = |text: &str| gtk::Label::builder().label(text).xalign(0.0).wrap(true).max_width_chars(28).build();
    let inputs: Vec<String> = task["reads"].as_array().into_iter().flatten().chain(task["urls"].as_array().into_iter().flatten()).filter_map(Value::as_str).map(str::to_owned).collect();
    let fetches = task["urls"].as_array().is_some_and(|urls| !urls.is_empty());

    let name = text(task["name"].as_str().unwrap_or_default());
    let name_entry = gtk::Entry::builder().text(task["name"].as_str().unwrap_or_default()).visible(false).build();
    let name_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    name_box.append(&name);
    name_box.append(&name_entry);
    add("Name", name_box.upcast_ref());
    add("Does", text(task["prompt"].as_str().unwrap_or_default()).upcast_ref());

    let when = text(&format!("{}, catching up after sleep", when_words(&task["when"])));
    let every = gtk::DropDown::from_strings(&EVERY.map(|(_, words)| words));
    let at = EVERY.iter().position(|(key, _)| Some(*key) == task["when"]["every"].as_str()).unwrap_or(0);
    every.set_selected(at as u32);
    let day = gtk::SpinButton::with_range(1.0, 31.0, 1.0);
    day.set_value(task["when"]["day"].as_f64().unwrap_or(1.0).max(1.0));
    day.set_visible(EVERY[at].0 == "month");
    let time = gtk::Entry::builder().text(task["when"]["time"].as_str().unwrap_or_default()).width_chars(6).max_width_chars(6).build();
    every.connect_selected_notify({
        let day = day.clone();
        move |every| day.set_visible(EVERY.get(every.selected() as usize).is_some_and(|(key, _)| *key == "month"))
    });
    let when_edit = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    when_edit.set_visible(false);
    when_edit.append(&every);
    when_edit.append(&day);
    when_edit.append(&gtk::Label::new(Some("at")));
    when_edit.append(&time);
    let when_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    when_box.append(&when);
    when_box.append(&when_edit);
    add("When", when_box.upcast_ref());

    let reads_words = if inputs.is_empty() { "Nothing but the instruction".to_owned() } else { inputs.join("\n") };
    let reads = text(&if fetches { format!("{reads_words}\nPages are fetched from the web on each run") } else { reads_words });
    let reads_entry = gtk::Entry::builder().text(inputs.join(", ")).placeholder_text("Files and https pages, with commas between").visible(false).build();
    let reads_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    reads_box.append(&reads);
    reads_box.append(&reads_entry);
    add("Reads", reads_box.upcast_ref());
    add("Runs on", text(&format!("This laptop (Ollama), up to {} minutes", task_minutes())).upcast_ref());
    add("Delivery", text("Notification card, banner only if urgent").upcast_ref());
    card.append(&grid);

    if state != "proposed" {
        let words = if state == "created" { created(unit, next) } else { "Not created".to_owned() };
        let outcome = gtk::Label::builder().label(words).xalign(0.0).wrap(true).max_width_chars(34).halign(gtk::Align::Start).build();
        outcome.add_css_class("assistant-outcome");
        card.append(&outcome);
        return card;
    }
    let create = gtk::Button::builder().label("Create task").focus_on_click(false).build();
    create.add_css_class("assistant-save");
    let edit = gtk::Button::builder().label("Edit").focus_on_click(false).build();
    edit.add_css_class("assistant-edit");
    let cancel = gtk::Button::builder().label("Cancel").focus_on_click(false).build();
    cancel.add_css_class("assistant-skip");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&create);
    buttons.append(&edit);
    buttons.append(&cancel);
    card.append(&buttons);
    let why = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(34).visible(false).build();
    why.add_css_class("assistant-error");
    card.append(&why);

    edit.connect_clicked({
        let (name, name_entry, when, when_edit, reads, reads_entry) = (name.clone(), name_entry.clone(), when.clone(), when_edit.clone(), reads.clone(), reads_entry.clone());
        move |edit| {
            for (shown, editor) in
                [(name.upcast_ref::<gtk::Widget>(), name_entry.upcast_ref::<gtk::Widget>()), (when.upcast_ref(), when_edit.upcast_ref()), (reads.upcast_ref(), reads_entry.upcast_ref())]
            {
                shown.set_visible(false);
                editor.set_visible(true);
            }
            name_entry.grab_focus();
            edit.set_visible(false);
        }
    });
    let finish = {
        let (card, buttons, why) = (card.clone(), buttons.clone(), why.clone());
        move |result: Result<String, String>| match result {
            Ok(words) => {
                buttons.set_visible(false);
                why.set_visible(false);
                let outcome = gtk::Label::builder().label(words).xalign(0.0).wrap(true).max_width_chars(34).halign(gtk::Align::Start).build();
                outcome.add_css_class("assistant-outcome");
                card.append(&outcome);
            }
            Err(error) => {
                why.set_label(&error);
                why.set_visible(true);
                buttons.set_sensitive(true);
            }
        }
    };
    create.connect_clicked({
        let (task, finish, buttons) = (task.clone(), finish.clone(), buttons.clone());
        let (name_entry, every, day, time, reads_entry) = (name_entry.clone(), every.clone(), day.clone(), time.clone(), reads_entry.clone());
        move |_| {
            let mut task = task.clone();
            // What the card shows is what's created, edits included.
            if WidgetExt::is_visible(&name_entry) {
                let (reads, urls): (Vec<String>, Vec<String>) =
                    reads_entry.text().split(',').map(|input| input.trim().to_owned()).filter(|input| !input.is_empty()).partition(|input| !input.contains("://"));
                task["name"] = Value::from(name_entry.text().trim());
                task["when"] = serde_json::json!({ "every": EVERY[every.selected() as usize].0, "day": day.value() as u64, "time": time.text().trim() });
                task["reads"] = Value::from(reads);
                task["urls"] = Value::from(urls);
            }
            buttons.set_sensitive(false);
            let finish = finish.clone();
            glib::spawn_future_local(async move {
                let created = async { ai::proxy().await?.create_task(entry, &task.to_string()).await }.await;
                finish(created.map(|(unit, next)| self::created(&unit, next)).map_err(said));
            });
        }
    });
    cancel.connect_clicked(move |_| {
        buttons.set_sensitive(false);
        let finish = finish.clone();
        glib::spawn_future_local(async move {
            let cancelled = async { ai::proxy().await?.cancel_task(entry).await }.await;
            finish(cancelled.map(|()| "Not created".to_owned()).map_err(said));
        });
    });
    card
}

fn outcome(saved: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(if saved { "Saved to ~/.config/autark-ai/memory.toml" } else { "Not saved" }));
    label.add_css_class("assistant-outcome");
    label.set_halign(gtk::Align::Start);
    label
}

/// The first label with `class` inside `widget`.
fn find_label(widget: &gtk::Widget, class: &str) -> gtk::Label {
    let mut stack = vec![widget.clone()];
    while let Some(widget) = stack.pop() {
        if widget.has_css_class(class)
            && let Ok(label) = widget.clone().downcast::<gtk::Label>()
        {
            return label;
        }
        let mut child = widget.first_child();
        while let Some(next) = child {
            child = next.next_sibling();
            stack.push(next);
        }
    }
    gtk::Label::new(None)
}

/// How many facts are in memory.toml, for the header.
fn memory_count() -> usize {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let path = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".config")).join("autark-ai/memory.toml");
    let document: toml_edit::DocumentMut = std::fs::read_to_string(path).unwrap_or_default().parse().unwrap_or_default();
    document
        .get("fact")
        .and_then(toml_edit::Item::as_array_of_tables)
        .map_or(0, |facts| facts.iter().filter(|fact| fact.get("text").and_then(toml_edit::Item::as_str).is_some_and(|text| !text.trim().is_empty())).count())
}

/// Settings' AI pane, where memory is corrected and forgotten.
fn open_settings() {
    let mut command = std::process::Command::new("autark-settings");
    command.arg("ai").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    match std::os::unix::process::CommandExt::process_group(&mut command, 0).spawn() {
        // Waited for, so it doesn't linger as a zombie once closed.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(err) => eprintln!("launcher: opening Settings: {err}"),
    }
}
