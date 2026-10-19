//! The mail window: folders on the left, the messages in the open folder, and the open message.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use autark_accounts::model::Account;
use autark_pim::imap::{Folder, Summary};
use autark_pim::mime::Message;
use chrono::Local;
use gtk::glib;
use gtk::prelude::*;

use crate::compose::{self, Draft};
use crate::{backend, logic};

/// The newest messages loaded for a folder.
const LOADED: usize = 100;

/// What a sidebar row stands for: an account's heading, or one of its folders.
#[derive(Clone)]
enum Entry {
    Heading,
    Folder { account: usize, folder: Folder },
}

pub struct Mail {
    pub window: gtk::ApplicationWindow,
    accounts: RefCell<Vec<Account>>,
    entries: RefCell<Vec<Entry>>,
    sidebar: gtk::ListBox,
    list: gtk::ListBox,
    reader: gtk::Box,
    status: gtk::Label,
    /// The open folder: the account's index and the folder's name.
    open_folder: RefCell<Option<(usize, String)>>,
    summaries: RefCell<Vec<Summary>>,
    /// The open message, with where it came from.
    open_message: RefCell<Option<(usize, String, u32, Message)>>,
    /// Bumped on each load, so a slow answer for something no longer shown is dropped.
    generation: Cell<u32>,
}

fn clear(container: &impl IsA<gtk::Widget>) {
    while let Some(child) = container.first_child() {
        child.unparent();
    }
}

impl Mail {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Mail").default_width(1280).default_height(780).build();
        window.add_css_class("mail");
        let header = gtk::HeaderBar::new();
        let new = gtk::Button::with_label("New message");
        new.add_css_class("primary");
        header.pack_start(&new);
        window.set_titlebar(Some(&header));

        let sidebar = gtk::ListBox::new();
        let sidebar_scroll = gtk::ScrolledWindow::builder().child(&sidebar).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
        status.add_css_class("status");
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.add_css_class("sidebar");
        left.set_size_request(220, -1);
        left.append(&sidebar_scroll);
        left.append(&status);

        let list = gtk::ListBox::new();
        let list_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let middle = gtk::Box::new(gtk::Orientation::Vertical, 0);
        middle.add_css_class("messages");
        middle.set_size_request(380, -1);
        middle.append(&list_scroll);

        let reader = gtk::Box::new(gtk::Orientation::Vertical, 8);
        reader.add_css_class("reader");
        reader.set_hexpand(true);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&left);
        root.append(&middle);
        root.append(&reader);
        window.set_child(Some(&root));

        let me = Rc::new(Self {
            window,
            accounts: RefCell::default(),
            entries: RefCell::default(),
            sidebar,
            list,
            reader,
            status,
            open_folder: RefCell::default(),
            summaries: RefCell::default(),
            open_message: RefCell::default(),
            generation: Cell::new(0),
        });
        let weak = Rc::downgrade(&me);
        me.sidebar.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let (Some(me), Some(row)) = (weak.upgrade(), row) else { return };
                if let Some(Entry::Folder { account, folder }) = me.entries.borrow().get(row.index() as usize).cloned() {
                    me.load_folder(account, folder.name);
                }
            }
        });
        me.list.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let (Some(me), Some(row)) = (weak.upgrade(), row) else { return };
                let summary = me.summaries.borrow().get(row.index() as usize).cloned();
                if let Some(summary) = summary {
                    me.load_message(summary);
                }
            }
        });
        new.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(me) = weak.upgrade() {
                    compose::open(&me, Draft::default());
                }
            }
        });
        me.draw_reader();
        me.load_accounts();
        me
    }

    pub fn accounts(&self) -> Vec<Account> {
        self.accounts.borrow().clone()
    }

    pub fn say(&self, text: &str, error: bool) {
        if error {
            self.status.add_css_class("error");
        } else {
            self.status.remove_css_class("error");
        }
        self.status.set_text(text);
    }

    /// Signs in to every account for its folders.
    fn load_accounts(self: &Rc<Self>) {
        self.say("Signing in…", false);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let loaded = blocking::unblock(|| {
                let accounts = backend::accounts()?;
                let folders: Vec<Result<Vec<Folder>, String>> = accounts.iter().map(backend::folders).collect();
                Ok::<_, String>((accounts, folders))
            })
            .await;
            let Some(me) = weak.upgrade() else { return };
            let (accounts, folders) = match loaded {
                Ok(loaded) => loaded,
                Err(why) => return me.say(&why, true),
            };
            if accounts.is_empty() {
                return me.say("No mail accounts yet. Add one: autark-accounts add-google", false);
            }
            let mut problems = Vec::new();
            let mut entries = Vec::new();
            me.sidebar.remove_all();
            let mut first_inbox = None;
            for (index, (account, folders)) in accounts.iter().zip(folders).enumerate() {
                let heading = gtk::Label::builder().label(&account.name).xalign(0.0).build();
                heading.add_css_class("account");
                me.sidebar.append(&gtk::ListBoxRow::builder().child(&heading).selectable(false).activatable(false).build());
                entries.push(Entry::Heading);
                match folders {
                    Ok(folders) => {
                        for folder in folders {
                            let row = gtk::ListBoxRow::builder().child(&gtk::Label::builder().label(logic::folder_title(&folder)).xalign(0.0).build()).build();
                            me.sidebar.append(&row);
                            if first_inbox.is_none() && folder.name.eq_ignore_ascii_case("inbox") {
                                first_inbox = Some(row);
                            }
                            entries.push(Entry::Folder { account: index, folder });
                        }
                    }
                    Err(why) => problems.push(why),
                }
            }
            *me.accounts.borrow_mut() = accounts;
            *me.entries.borrow_mut() = entries;
            if problems.is_empty() {
                me.say("", false);
            } else {
                me.say(&problems.join("  ·  "), true);
            }
            if let Some(row) = first_inbox {
                me.sidebar.select_row(Some(&row));
            }
        });
    }

    fn load_folder(self: &Rc<Self>, account: usize, folder: String) {
        let Some(chosen) = self.accounts.borrow().get(account).cloned() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        *self.open_folder.borrow_mut() = Some((account, folder.clone()));
        *self.open_message.borrow_mut() = None;
        self.say("Loading…", false);
        self.draw_reader();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let name = folder.clone();
            let loaded = blocking::unblock(move || backend::summaries(&chosen, &name, LOADED)).await;
            let Some(me) = weak.upgrade() else { return };
            if me.generation.get() != generation {
                return;
            }
            match loaded {
                Ok(list) => {
                    me.say(&format!("{} message{}", list.len(), if list.len() == 1 { "" } else { "s" }), false);
                    *me.summaries.borrow_mut() = list;
                    me.draw_list();
                }
                Err(why) => me.say(&why, true),
            }
        });
    }

    fn draw_list(self: &Rc<Self>) {
        self.list.remove_all();
        let today = Local::now().date_naive();
        for summary in self.summaries.borrow().iter() {
            let line = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let from = gtk::Label::builder().label(if summary.from.is_empty() { "(no sender)" } else { &summary.from }).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
            from.add_css_class("from");
            let date = gtk::Label::builder().label(logic::short_date(&summary.date, today)).build();
            date.add_css_class("date");
            top.append(&from);
            top.append(&date);
            let subject = gtk::Label::builder().label(if summary.subject.is_empty() { "(no subject)" } else { &summary.subject }).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
            subject.add_css_class("subject");
            line.append(&top);
            line.append(&subject);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            if !summary.is_seen() {
                row.add_css_class("unread");
            }
            self.list.append(&row);
        }
    }

    fn load_message(self: &Rc<Self>, summary: Summary) {
        let Some((account, folder)) = self.open_folder.borrow().clone() else { return };
        let Some(chosen) = self.accounts.borrow().get(account).cloned() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let name = folder.clone();
            let uid = summary.uid;
            let loaded = blocking::unblock(move || backend::read(&chosen, &name, uid)).await;
            let Some(me) = weak.upgrade() else { return };
            if me.generation.get() != generation {
                return;
            }
            match loaded {
                Ok(message) => {
                    *me.open_message.borrow_mut() = Some((account, folder, uid, message));
                    // Opening marks it read, in the list too.
                    if let Some(row) = me.summaries.borrow_mut().iter_mut().find(|other| other.uid == uid)
                        && !row.is_seen()
                    {
                        row.flags.push("\\Seen".to_owned());
                    }
                    if let Some(index) = me.summaries.borrow().iter().position(|other| other.uid == uid)
                        && let Some(row) = me.list.row_at_index(index as i32)
                    {
                        row.remove_css_class("unread");
                    }
                    me.draw_reader();
                }
                Err(why) => me.say(&why, true),
            }
        });
    }

    fn draw_reader(self: &Rc<Self>) {
        clear(&self.reader);
        let opened = self.open_message.borrow().clone();
        let Some((account, folder, uid, message)) = opened else {
            let empty = gtk::Label::builder().label("Select a message to read.").xalign(0.0).build();
            empty.add_css_class("empty");
            self.reader.append(&empty);
            return;
        };
        let subject = gtk::Label::builder().label(if message.subject.is_empty() { "(no subject)" } else { &message.subject }).xalign(0.0).wrap(true).selectable(true).build();
        subject.add_css_class("subject");
        self.reader.append(&subject);
        let people = |list: &[autark_pim::mime::Address]| list.iter().map(|a| if a.name.is_empty() { a.email.clone() } else { format!("{} <{}>", a.name, a.email) }).collect::<Vec<_>>().join(", ");
        for (label, text) in [("From", people(&message.from)), ("To", people(&message.to)), ("Cc", people(&message.cc)), ("Date", logic::long_date(&message.date))] {
            if text.is_empty() {
                continue;
            }
            let line = gtk::Label::builder().label(format!("{label}: {text}")).xalign(0.0).wrap(true).selectable(true).build();
            line.add_css_class("meta");
            self.reader.append(&line);
        }

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_margin_top(6);
        let mine = self.accounts.borrow().get(account).map(|a| a.email.clone()).unwrap_or_default();
        for (label, mode) in [("Reply", 0), ("Reply all", 1), ("Forward", 2)] {
            let button = gtk::Button::with_label(label);
            let (weak, message, mine) = (Rc::downgrade(self), message.clone(), mine.clone());
            button.connect_clicked(move |_| {
                let Some(me) = weak.upgrade() else { return };
                let draft = match mode {
                    2 => Draft { account, subject: logic::forward_subject(&message.subject), body: format!("\n\n---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", message.from.first().map(|a| a.email.clone()).unwrap_or_default(), logic::long_date(&message.date), message.subject, logic::body_text(&message)), ..Default::default() },
                    all => {
                        let (to, cc) = logic::reply_to(&message, &mine, all == 1);
                        let join = |list: &[autark_pim::mime::Address]| list.iter().map(|a| a.email.clone()).collect::<Vec<_>>().join(", ");
                        Draft { account, to: join(&to), cc: join(&cc), subject: logic::reply_subject(&message.subject), body: logic::quote(&message), in_reply_to: message.message_id.clone() }
                    }
                };
                compose::open(&me, draft);
            });
            buttons.append(&button);
        }
        let delete = gtk::Button::with_label("Delete");
        delete.add_css_class("danger");
        buttons.append(&delete);
        let weak = Rc::downgrade(self);
        delete.connect_clicked(move |button| {
            let Some(me) = weak.upgrade() else { return };
            let Some(chosen) = me.accounts.borrow().get(account).cloned() else { return };
            button.set_sensitive(false);
            let folder = folder.clone();
            glib::spawn_future_local(async move {
                let (name, from_folder) = (folder.clone(), folder.clone());
                match blocking::unblock(move || backend::trash(&chosen, &name, uid)).await {
                    Ok(()) => {
                        me.say("Moved to the Trash.", false);
                        me.load_folder(account, from_folder);
                    }
                    Err(why) => me.say(&why, true),
                }
            });
        });
        self.reader.append(&buttons);

        if !message.attachments.is_empty() {
            let files = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            files.set_margin_top(6);
            for attachment in &message.attachments {
                let button = gtk::Button::with_label(&format!("{}  ({} KB)", attachment.name, attachment.data.len().div_ceil(1024)));
                let (weak, attachment) = (Rc::downgrade(self), attachment.clone());
                button.connect_clicked(move |_| {
                    let Some(me) = weak.upgrade() else { return };
                    match save(&attachment.name, &attachment.data) {
                        Ok(path) => me.say(&format!("Saved {}", path.display()), false),
                        Err(why) => me.say(&why, true),
                    }
                });
                files.append(&button);
            }
            self.reader.append(&files);
        }

        let body = gtk::TextView::builder().editable(false).cursor_visible(false).wrap_mode(gtk::WrapMode::Word).vexpand(true).build();
        body.buffer().set_text(&logic::body_text(&message));
        self.reader.append(&gtk::ScrolledWindow::builder().child(&body).vexpand(true).margin_top(10).build());
    }
}

/// Saves an attachment in Downloads under its own name (only the file name part), never over another file.
fn save(name: &str, data: &[u8]) -> Result<std::path::PathBuf, String> {
    let base = std::path::Path::new(name).file_name().map(|name| name.to_string_lossy().into_owned()).filter(|name| !name.is_empty() && !name.starts_with('.')).unwrap_or_else(|| "attachment".to_owned());
    let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let (stem, extension) = match base.rsplit_once('.') {
        Some((stem, extension)) => (stem.to_owned(), format!(".{extension}")),
        None => (base.clone(), String::new()),
    };
    for n in 0..1000 {
        let file = if n == 0 { base.clone() } else { format!("{stem} ({n}){extension}") };
        let path = dir.join(file);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut out) => {
                std::io::Write::write_all(&mut out, data).map_err(|err| format!("{}: {err}", path.display()))?;
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("{}: {err}", path.display())),
        }
    }
    Err("too many files with that name".to_owned())
}
