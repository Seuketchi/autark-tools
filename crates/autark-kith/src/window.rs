//! The team's window: the roster on the left, with a face, the last words and the time for each agent, and the
//! conversation with the one picked on the right. A new answer grows in its bubble as `autark-ai` writes it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use futures_util::StreamExt;
use gtk::prelude::*;
use gtk::{glib, pango};

use crate::ai;
use crate::logic::{self, Member, Message, Who};

/// What `autark-ai` tells the window.
enum Event {
    /// An agent's conversation changed.
    Agent(String),
    /// The team changed.
    Team,
    Piece(u32, u32, String),
}

pub struct Kith {
    pub window: gtk::ApplicationWindow,
    list: gtk::ListBox,
    search: gtk::SearchEntry,
    members: RefCell<Vec<Member>>,
    current: RefCell<Option<String>>,
    /// The agent whose name and conversation are on show.
    shown: RefCell<Option<String>>,
    /// Set while the list is rebuilt, so putting the selection back isn't taken for a click.
    rebuilding: Cell<bool>,
    face: gtk::DrawingArea,
    face_colour: Rc<Cell<u8>>,
    title: gtk::Label,
    job: gtk::Label,
    messages: gtk::Box,
    scroll: gtk::ScrolledWindow,
    entry: gtk::Entry,
    send: gtk::Button,
    note: gtk::Label,
    /// The bubbles being written, by answer id, with what they hold so far.
    writing: RefCell<HashMap<u32, (gtk::Label, String)>>,
}

/// A round face in the agent's colour with two eyes, the way Kith's own looks.
fn face(colour: Rc<Cell<u8>>, size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(size).content_height(size).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, width, height| {
        let (w, h) = (f64::from(width), f64::from(height));
        let [r, g, b] = autark_common::theme::rgb(logic::colour(colour.get()));
        cr.set_source_rgb(r, g, b);
        cr.arc(w / 2.0, h / 2.0, w.min(h) / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_source_rgba(0.08, 0.08, 0.08, 0.85);
        for at in [0.36, 0.64] {
            cr.save().ok();
            cr.translate(w * at, h * 0.45);
            cr.scale(w * 0.055, h * 0.11);
            cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
            cr.restore().ok();
            let _ = cr.fill();
        }
    });
    area
}

/// When, for the roster: the time today, "Yesterday", or the date.
fn short_time(time: i64) -> String {
    if time <= 0 {
        return String::new();
    }
    let (Ok(then), Ok(now)) = (glib::DateTime::from_unix_local(time), glib::DateTime::now_local()) else {
        return String::new();
    };
    let day = |date: &glib::DateTime| date.ymd();
    let format = |pattern: &str| then.format(pattern).map(|text| text.to_string()).unwrap_or_default();
    if day(&then) == day(&now) {
        format("%-I:%M %p")
    } else if now.add_days(-1).is_ok_and(|yesterday| day(&yesterday) == day(&then)) {
        "Yesterday".to_owned()
    } else {
        format("%b %-d")
    }
}

/// The stamp between messages: the time today, otherwise with the date.
fn long_time(time: i64) -> String {
    let short = short_time(time);
    match (glib::DateTime::from_unix_local(time), glib::DateTime::now_local()) {
        (Ok(then), Ok(now)) if then.ymd() != now.ymd() => format!("{short}, {}", then.format("%-I:%M %p").map(|text| text.to_string()).unwrap_or_default()),
        _ => short,
    }
}

impl Kith {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Kith").default_width(1000).default_height(700).build();
        window.add_css_class("kith");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        // The roster.
        let heading = gtk::Label::builder().label("Team").xalign(0.0).hexpand(true).build();
        heading.add_css_class("heading");
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("New agent").build();
        add.add_css_class("add");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        head.add_css_class("roster-head");
        head.append(&heading);
        head.append(&add);
        let search = gtk::SearchEntry::builder().placeholder_text("Search").build();
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        let roster_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let roster = gtk::Box::new(gtk::Orientation::Vertical, 0);
        roster.add_css_class("roster");
        roster.set_width_request(320);
        // A row's text expands inside it, which would pass up and let the roster grow beyond its width.
        roster.set_hexpand(false);
        roster.append(&head);
        roster.append(&search);
        roster.append(&roster_scroll);

        // The conversation.
        let face_colour = Rc::new(Cell::new(0));
        let head_face = face(face_colour.clone(), 38);
        let title = gtk::Label::builder().xalign(0.0).build();
        title.add_css_class("title");
        let job = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        job.add_css_class("job");
        let names = gtk::Box::new(gtk::Orientation::Vertical, 1);
        names.set_hexpand(true);
        names.set_valign(gtk::Align::Center);
        names.append(&title);
        names.append(&job);
        let more = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("This agent").build();
        more.add_css_class("more");
        let controls = autark_common::chrome::controls(true);
        let chat_head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        chat_head.add_css_class("chat-head");
        chat_head.append(&head_face);
        chat_head.append(&names);
        chat_head.append(&more);
        chat_head.append(&controls);
        let handle = gtk::WindowHandle::builder().child(&chat_head).build();

        let messages = gtk::Box::new(gtk::Orientation::Vertical, 0);
        messages.add_css_class("messages");
        let scroll = gtk::ScrolledWindow::builder().child(&messages).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let note = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        note.add_css_class("note");
        let entry = gtk::Entry::builder().hexpand(true).build();
        let send = gtk::Button::builder().icon_name("go-up-symbolic").tooltip_text("Send").sensitive(false).build();
        send.add_css_class("send");
        let compose = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        compose.add_css_class("compose");
        compose.append(&entry);
        compose.append(&send);
        let chat = gtk::Box::new(gtk::Orientation::Vertical, 0);
        chat.set_hexpand(true);
        chat.append(&handle);
        chat.append(&scroll);
        chat.append(&note);
        chat.append(&compose);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&roster);
        root.append(&chat);
        window.set_child(Some(&root));

        let kith = Rc::new(Self {
            window,
            list,
            search,
            members: RefCell::default(),
            current: RefCell::default(),
            shown: RefCell::default(),
            rebuilding: Cell::new(false),
            face: head_face,
            face_colour,
            title,
            job,
            messages,
            scroll,
            entry,
            send,
            note,
            writing: RefCell::default(),
        });
        kith.wire(&add, &more);
        kith.refresh();
        kith.listen();
        kith
    }

    fn wire(self: &Rc<Self>, add: &gtk::Button, more: &gtk::MenuButton) {
        let kith = self.clone();
        self.list.connect_row_selected(move |_, row| {
            if kith.rebuilding.get() {
                return;
            }
            if let Some(row) = row {
                kith.pick(row.widget_name().as_str());
            }
        });
        let kith = self.clone();
        self.search.connect_search_changed(move |_| kith.fill());
        let kith = self.clone();
        self.entry.connect_activate(move |_| kith.say());
        let kith = self.clone();
        self.send.connect_clicked(move |_| kith.say());
        let kith = self.clone();
        self.entry.connect_changed(move |entry| kith.send.set_sensitive(!entry.text().trim().is_empty()));
        let kith = self.clone();
        add.connect_clicked(move |_| kith.new_agent());

        // The agent's own menu: clear the conversation, or remove the agent.
        let menu = gtk::gio::Menu::new();
        menu.append(Some("Clear conversation"), Some("kith.clear"));
        menu.append(Some("Delete agent"), Some("kith.delete"));
        more.set_menu_model(Some(&menu));
        let actions = gtk::gio::SimpleActionGroup::new();
        let clear = gtk::gio::SimpleAction::new("clear", None);
        let kith = self.clone();
        clear.connect_activate(move |_, _| kith.clear());
        actions.add_action(&clear);
        let delete = gtk::gio::SimpleAction::new("delete", None);
        let kith = self.clone();
        delete.connect_activate(move |_, _| kith.confirm_delete());
        actions.add_action(&delete);
        self.window.insert_action_group("kith", Some(&actions));
    }

    fn fail(&self, text: &str) {
        self.note.set_label(text);
        self.note.set_visible(!text.is_empty());
    }

    /// Reads the team again and shows it.
    fn refresh(self: &Rc<Self>) {
        let kith = self.clone();
        glib::spawn_future_local(async move {
            match async { ai::proxy().await?.agents().await }.await {
                Ok(json) => {
                    *kith.members.borrow_mut() = logic::team(&json);
                    kith.fail("");
                    kith.fill();
                    // The agent to show: the one picked or just made, else Kith, the first.
                    let wanted = {
                        let members = kith.members.borrow();
                        let current = kith.current.borrow();
                        current.clone().filter(|id| members.iter().any(|member| &member.id == id)).or_else(|| members.first().map(|member| member.id.clone()))
                    };
                    if let Some(wanted) = wanted
                        && kith.shown.borrow().as_deref() != Some(wanted.as_str())
                    {
                        kith.pick(&wanted);
                    }
                }
                Err(err) => kith.fail(&ai::said(&err)),
            }
        });
    }

    /// Builds the roster's rows from the team and what is typed in the search.
    fn fill(&self) {
        self.rebuilding.set(true);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let query = self.search.text().to_string();
        let current = self.current.borrow().clone();
        for member in self.members.borrow().iter().filter(|member| logic::matches(member, &query)) {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("member");
            row.set_widget_name(&member.id);
            let name = gtk::Label::builder().label(member.name.as_str()).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            name.add_css_class("name");
            let when = gtk::Label::builder().label(short_time(member.time)).build();
            when.add_css_class("when");
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            top.append(&name);
            top.append(&when);
            let said = if member.writing { "Typing…".to_owned() } else { logic::preview(&member.last) };
            let preview = gtk::Label::builder().label(said).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            preview.add_css_class("preview");
            if member.writing {
                preview.add_css_class("typing");
            }
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_hexpand(true);
            text.append(&top);
            text.append(&preview);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            line.append(&face(Rc::new(Cell::new(member.colour)), 40));
            line.append(&text);
            row.set_child(Some(&line));
            self.list.append(&row);
            if current.as_deref() == Some(member.id.as_str()) {
                self.list.select_row(Some(&row));
            }
        }
        self.rebuilding.set(false);
    }

    /// Shows an agent's conversation.
    fn pick(self: &Rc<Self>, id: &str) {
        let member = self.members.borrow().iter().find(|member| member.id == id).cloned();
        let Some(member) = member else { return };
        *self.current.borrow_mut() = Some(member.id.clone());
        *self.shown.borrow_mut() = Some(member.id.clone());
        self.face_colour.set(member.colour);
        self.face.queue_draw();
        self.title.set_label(&member.name);
        self.job.set_label(&logic::preview(&member.job));
        self.entry.set_placeholder_text(Some(&format!("Message {}", member.name)));
        self.fill();
        self.reload();
        self.entry.grab_focus();
    }

    /// Reads the open conversation again and shows it.
    fn reload(self: &Rc<Self>) {
        let Some(id) = self.current.borrow().clone() else {
            return;
        };
        let kith = self.clone();
        glib::spawn_future_local(async move {
            let json = async { ai::proxy().await?.agent_history(&id).await }.await;
            if kith.current.borrow().as_deref() != Some(id.as_str()) {
                return;
            }
            match json {
                Ok(json) => {
                    kith.fail("");
                    kith.show(&logic::messages(&json));
                }
                Err(err) => kith.fail(&ai::said(&err)),
            }
        });
    }

    fn show(&self, messages: &[Message]) {
        while let Some(child) = self.messages.first_child() {
            self.messages.remove(&child);
        }
        self.writing.borrow_mut().clear();
        if messages.is_empty() {
            let name = self.title.label();
            let empty = gtk::Label::builder().label(format!("Say something to {name} to begin.")).vexpand(true).valign(gtk::Align::Center).build();
            empty.add_css_class("empty");
            self.messages.append(&empty);
            return;
        }
        let mut before = None;
        for message in messages {
            if logic::stamp_between(before, message.time) {
                let stamp = gtk::Label::builder().label(long_time(message.time)).build();
                stamp.add_css_class("stamp");
                self.messages.append(&stamp);
            }
            before = Some(message.time);
            let text = if message.error.is_empty() { message.text.clone() } else { message.error.clone() };
            let shown = if message.writing.is_some() { format!("{text}▍") } else { text.clone() };
            let bubble = gtk::Label::builder()
                .label(shown)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .max_width_chars(62)
                .selectable(true)
                .xalign(0.0)
                .halign(if message.who == Who::You { gtk::Align::End } else { gtk::Align::Start })
                .build();
            bubble.add_css_class("bubble");
            bubble.add_css_class(if message.who == Who::You { "you" } else { "agent" });
            if !message.error.is_empty() {
                bubble.add_css_class("failed");
            }
            if let Some(answer) = message.writing {
                self.writing.borrow_mut().insert(answer, (bubble.clone(), message.text.clone()));
            }
            self.messages.append(&bubble);
        }
        self.follow_bottom();
    }

    /// Scrolls to the newest message once the window has laid it out.
    fn follow_bottom(&self) {
        let adjustment = self.scroll.vadjustment();
        glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper() - adjustment.page_size()));
    }

    fn piece(&self, id: u32, offset: u32, piece: &str) {
        let mut writing = self.writing.borrow_mut();
        let Some((label, text)) = writing.get_mut(&id) else {
            return;
        };
        logic::merge(text, offset, piece);
        label.set_label(&format!("{text}▍"));
        drop(writing);
        self.follow_bottom();
    }

    fn say(self: &Rc<Self>) {
        let text = self.entry.text().trim().to_owned();
        let Some(id) = self.current.borrow().clone() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if self.members.borrow().iter().any(|member| member.id == id && member.writing) || !self.writing.borrow().is_empty() {
            return;
        }
        self.entry.set_text("");
        let kith = self.clone();
        glib::spawn_future_local(async move {
            match async { ai::proxy().await?.agent_say(&id, &text).await }.await {
                Ok(_) => kith.fail(""),
                Err(err) => kith.fail(&ai::said(&err)),
            }
        });
    }

    fn clear(self: &Rc<Self>) {
        let Some(id) = self.current.borrow().clone() else {
            return;
        };
        let kith = self.clone();
        glib::spawn_future_local(async move {
            if let Err(err) = async { ai::proxy().await?.clear_agent(&id).await }.await {
                kith.fail(&ai::said(&err));
            }
        });
    }

    fn confirm_delete(self: &Rc<Self>) {
        let Some(id) = self.current.borrow().clone() else {
            return;
        };
        if id == "kith" {
            self.fail("Kith can't be deleted.");
            return;
        }
        let name = self.title.label();
        let dialog = gtk::AlertDialog::builder().message(format!("Delete {name}?")).detail("Its conversation goes with it.").buttons(["Cancel", "Delete"]).cancel_button(0).default_button(0).build();
        let kith = self.clone();
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&window)).await != Ok(1) {
                return;
            }
            if let Err(err) = async { ai::proxy().await?.delete_agent(&id).await }.await {
                kith.fail(&ai::said(&err));
            }
        });
    }

    /// A small window to name an agent, say what it is for and how it sounds.
    fn new_agent(self: &Rc<Self>) {
        let dialog = gtk::Window::builder()
            .application(&self.window.application().expect("the window belongs to the app"))
            .transient_for(&self.window)
            .modal(true)
            .title("New agent")
            .default_width(420)
            .build();
        dialog.add_css_class("new-agent");
        let form = gtk::Box::new(gtk::Orientation::Vertical, 4);
        form.add_css_class("form");
        let label = |text: &str| {
            let label = gtk::Label::builder().label(text).xalign(0.0).build();
            label.add_css_class("field-label");
            label
        };
        let name = gtk::Entry::builder().placeholder_text("Inbox Manager").max_length(40).build();
        let job = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).height_request(90).build();
        let soul = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).height_request(70).build();
        let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        error.add_css_class("note");
        let create = gtk::Button::builder().label("Create").halign(gtk::Align::End).margin_top(12).build();
        create.add_css_class("suggested-action");
        form.append(&label("Name"));
        form.append(&name);
        form.append(&label("What is it for? Say it as an instruction to the agent."));
        form.append(&job);
        form.append(&label("How should it sound? Optional."));
        form.append(&soul);
        form.append(&label("What may it look at? It only reads; it never sends or changes anything."));
        let can = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).max_children_per_line(3).homogeneous(true).build();
        let boxes: Vec<(&str, gtk::CheckButton)> = [("mail", "Mail"), ("calendar", "Calendar"), ("tasks", "Tasks"), ("notes", "Notes"), ("web", "Web"), ("laptop", "Laptop")]
            .into_iter()
            .map(|(key, title)| {
                let check = gtk::CheckButton::with_label(title);
                can.append(&check);
                (key, check)
            })
            .collect();
        form.append(&can);
        form.append(&label("Skills it works by, by name and separated by commas. Optional."));
        let skills = gtk::Entry::builder().placeholder_text("accountant").build();
        form.append(&skills);
        form.append(&error);
        form.append(&create);
        dialog.set_child(Some(&form));
        let text = |view: &gtk::TextView| {
            let buffer = view.buffer();
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
        };
        let kith = self.clone();
        let window = dialog.clone();
        create.connect_clicked(move |_| {
            let (name, job, soul) = (name.text().to_string(), text(&job), text(&soul));
            let skills_text = skills.text().to_string();
            let skills: Vec<String> = skills_text.split(',').map(str::trim).filter(|skill| !skill.is_empty()).map(str::to_owned).collect();
            let can: Vec<&'static str> = boxes.iter().filter(|(_, check)| check.is_active()).map(|(key, _)| *key).collect();
            let (kith, window, error) = (kith.clone(), window.clone(), error.clone());
            glib::spawn_future_local(async move {
                match async { ai::proxy().await?.create_agent(&name, &job, &soul, &can, &skills.iter().map(String::as_str).collect::<Vec<_>>()).await }.await {
                    Ok(id) => {
                        window.close();
                        *kith.current.borrow_mut() = Some(id);
                    }
                    Err(err) => {
                        error.set_label(&ai::said(&err));
                        error.set_visible(true);
                    }
                }
            });
        });
        dialog.present();
    }

    /// Follows `autark-ai` while the window is open: an agent's words as they're written, and changes to the team.
    /// Tries again every few seconds when `autark-ai` isn't running, so starting it brings the team in.
    fn listen(self: &Rc<Self>) {
        let kith = self.clone();
        glib::spawn_future_local(async move {
            loop {
                let result = async {
                    let proxy = ai::proxy().await?;
                    let changed = proxy.receive_agent_changed().await?.filter_map(|signal| std::future::ready(signal.args().ok().map(|args| Event::Agent(args.id))));
                    let kiths = proxy.receive_history_changed().await?.map(|_| Event::Agent("kith".to_owned()));
                    let team = proxy.receive_agents_changed().await?.map(|_| Event::Team);
                    let pieces = proxy.receive_answer_piece().await?.filter_map(|signal| std::future::ready(signal.args().ok().map(|args| Event::Piece(args.id, args.offset, args.text))));
                    let mut events = futures_util::stream::select(futures_util::stream::select(changed, kiths), futures_util::stream::select(team, pieces));
                    while let Some(event) = events.next().await {
                        match event {
                            Event::Agent(id) => {
                                kith.refresh();
                                if kith.current.borrow().as_deref() == Some(id.as_str()) {
                                    kith.reload();
                                }
                            }
                            Event::Team => kith.refresh(),
                            Event::Piece(id, offset, text) => kith.piece(id, offset, &text),
                        }
                    }
                    zbus::Result::Ok(())
                };
                if let Err(err) = result.await {
                    eprintln!("kith: following the team: {err}");
                }
                glib::timeout_future(Duration::from_secs(3)).await;
                kith.refresh();
            }
        });
    }
}
