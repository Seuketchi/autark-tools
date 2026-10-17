//! The pinned note: the first note pinned in Notes, or `note` in notifyd.toml. Shows its
//! checklist, or its first lines if it has none. Ticking a box on the desktop writes the file,
//! and Notes picks the change up like any other edit made outside it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use serde::Deserialize;

use crate::shared::Shared;

#[derive(Clone, PartialEq, Debug)]
pub struct Note {
    pub path: PathBuf,
    pub title: String,
    pub items: Vec<Item>,
    /// The first lines, for a note without a checklist.
    pub lines: Vec<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Item {
    pub text: String,
    pub done: bool,
    /// Where it is in the file, counting from 0.
    pub line: usize,
}

impl Note {
    /// "2 of 4 done  ·  next: Write the daemon", or the first line.
    pub fn summary(&self) -> String {
        if self.items.is_empty() {
            return self.lines.first().cloned().unwrap_or_else(|| "Empty note".to_owned());
        }
        let done = self.items.iter().filter(|item| item.done).count();
        match self.items.iter().find(|item| !item.done) {
            Some(next) => format!("{done} of {} done  ·  next: {}", self.items.len(), next.text),
            None => format!("All {} done", self.items.len()),
        }
    }
}

/// Reads the note's text into a `Note`.
pub fn parse(path: PathBuf, title: &str, text: &str) -> Note {
    let mut items = Vec::new();
    let mut lines = Vec::new();
    let mut fenced = false;
    let mut front_matter = text.starts_with("---\n");
    for (number, line) in text.lines().enumerate() {
        if front_matter {
            if number > 0 && line == "---" {
                front_matter = false;
            }
            continue;
        }
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some((done, rest)) = checkbox(line) {
            items.push(Item { text: plain(rest), done, line: number });
            continue;
        }
        let shown = plain(line.trim().trim_start_matches(['#', '>']).trim_start_matches("- ").trim());
        if !shown.is_empty() && shown != title && lines.len() < 4 {
            lines.push(shown);
        }
    }
    Note { path, title: title.to_owned(), items, lines }
}

/// `- [ ] text` or `- [x] text`, with `*` or `+` for the dash and any indent.
fn checkbox(line: &str) -> Option<(bool, &str)> {
    let rest = line.trim_start().strip_prefix(['-', '*', '+'])?.strip_prefix(' ')?;
    if let Some(rest) = rest.strip_prefix("[ ] ") {
        return Some((false, rest));
    }
    rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")).map(|rest| (true, rest))
}

/// Markdown markers dropped and links shown as their names.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else {
            break;
        };
        let inner = &rest[..end];
        out.push_str(inner.rsplit('|').next().unwrap_or(inner));
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    out.replace("**", "").replace(['`', '*'], "").trim().to_owned()
}

/// Flips one box in the file, if the line still holds the same item.
pub fn toggle(note: &Note, item: &Item) -> std::io::Result<()> {
    let text = std::fs::read_to_string(&note.path)?;
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some(line) = lines.get(item.line).copied() else {
        return Ok(());
    };
    if checkbox(line.trim_end_matches(['\n', '\r'])).is_none_or(|(done, rest)| done != item.done || plain(rest) != item.text) {
        return Ok(());
    }
    let (from, to) = if item.done { (if line.contains("[x] ") { "[x] " } else { "[X] " }, "[ ] ") } else { ("[ ] ", "[x] ") };
    let flipped = line.replacen(from, to, 1);
    lines[item.line] = &flipped;
    write_atomically(&note.path, &lines.concat())
}

/// Through a temporary file beside it, as Notes saves, so a reader never sees half a note.
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.saving"));
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NotesConfig {
    vault: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NotesState {
    pinned: Vec<String>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// The vault Notes opens: `vault` in notes.toml, the last one opened, then `~/Notes`.
fn vault() -> Option<PathBuf> {
    let configured = autark_common::config::load::<NotesConfig>("notes").vault.map(|vault| match vault.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(vault),
    });
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"));
    let last = std::fs::read_to_string(state.join("autark/notes-vault")).ok().map(|text| PathBuf::from(text.trim()));
    [configured, last, Some(home().join("Notes"))].into_iter().flatten().find(|path| path.is_dir())
}

fn state_file(vault: &Path) -> PathBuf {
    vault.join(".autark-notes/state.toml")
}

/// Keeps `note` current: rereads it when the file or Notes' pins change.
pub struct Source {
    pub note: Shared<Option<Note>>,
    configured: Option<String>,
    monitors: RefCell<Vec<(PathBuf, gio::FileMonitor)>>,
    pending: RefCell<Option<glib::SourceId>>,
    me: Weak<Source>,
}

impl Source {
    pub fn new(configured: Option<String>) -> Rc<Self> {
        let source = Rc::new_cyclic(|me| Self { note: Shared::new(None), configured, monitors: RefCell::default(), pending: RefCell::default(), me: me.clone() });
        source.reload();
        source
    }

    pub fn reload(&self) {
        let vault = vault();
        let found = vault.as_deref().and_then(|vault| {
            let path = match &self.configured {
                Some(path) => path.clone(),
                None => {
                    let text = std::fs::read_to_string(state_file(vault)).ok()?;
                    toml::from_str::<NotesState>(&text).ok()?.pinned.into_iter().next()?
                }
            };
            let path = vault.join(path);
            let text = std::fs::read_to_string(&path).ok()?;
            let title = path.file_stem()?.to_string_lossy().into_owned();
            Some(parse(path, &title, &text))
        });
        let watched: Vec<PathBuf> = vault.iter().map(|vault| state_file(vault)).chain(found.iter().map(|note| note.path.clone())).collect();
        if self.note.get() != found {
            self.note.set(found);
        }
        self.watch(&watched);
    }

    fn watch(&self, paths: &[PathBuf]) {
        let mut monitors = self.monitors.borrow_mut();
        if monitors.iter().map(|(path, _)| path).eq(paths) {
            return;
        }
        for (_, monitor) in monitors.drain(..) {
            monitor.cancel();
        }
        for path in paths {
            let Ok(monitor) = gio::File::for_path(path).monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) else {
                continue;
            };
            let me = self.me.clone();
            monitor.connect_changed(move |_, _, _, _| {
                if let Some(source) = me.upgrade() {
                    source.reload_soon();
                }
            });
            monitors.push((path.clone(), monitor));
        }
    }

    /// Saves come as a burst of events; one reread after they settle is enough.
    fn reload_soon(&self) {
        if let Some(pending) = self.pending.take() {
            pending.remove();
        }
        let me = self.me.clone();
        *self.pending.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_millis(200), move || {
            if let Some(source) = me.upgrade() {
                source.pending.take();
                source.reload();
            }
        }));
    }
}

fn open_notes() {
    if let Err(err) = std::process::Command::new("autark-notes").spawn() {
        eprintln!("notifyd: opening Notes: {err}");
    }
}

fn on_click(widget: &impl IsA<gtk::Widget>, action: impl Fn() + 'static) {
    let click = gtk::GestureClick::new();
    click.connect_released(move |_, _, _, _| action());
    widget.add_controller(click);
    widget.set_cursor_from_name(Some("pointer"));
}

/// The centre's card: title and a one-line summary. Clicking opens Notes.
pub fn view(note: &Shared<Option<Note>>) -> gtk::Box {
    let title = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
    title.add_css_class("note-title");
    let summary = gtk::Label::builder().xalign(0.0).wrap(true).build();
    summary.add_css_class("detail");
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("card");
    card.add_css_class("widget");
    card.append(&super::caption("PINNED NOTE"));
    card.append(&title);
    card.append(&summary);
    on_click(&card, open_notes);
    note.watch(&card, move |_, note| match note {
        Some(note) => {
            title.set_visible(true);
            title.set_label(&note.title);
            summary.set_label(&note.summary());
        }
        None => {
            title.set_visible(false);
            summary.set_label("Pin a note in Notes to see it here.");
        }
    });
    card
}

/// The desktop's card: the checklist with boxes you can tick, or the first lines.
pub fn desktop_view(note: &Shared<Option<Note>>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("card");
    card.add_css_class("note");
    card.add_css_class("large");
    let caption = super::caption("PINNED NOTE");
    let title = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
    title.add_css_class("note-title");
    on_click(&title, open_notes);
    let items = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.append(&caption);
    card.append(&title);
    card.append(&items);
    note.watch(&card, move |_, note| {
        let Some(note) = note else { return };
        title.set_label(&note.title);
        while let Some(child) = items.first_child() {
            items.remove(&child);
        }
        for item in note.items.iter().take(8) {
            items.append(&check_row(note, item));
        }
        for line in note.lines.iter().take(if note.items.is_empty() { 4 } else { 0 }) {
            let label = gtk::Label::builder().label(line).xalign(0.0).wrap(true).build();
            label.add_css_class("line");
            items.append(&label);
        }
    });
    card
}

fn check_row(note: &Note, item: &Item) -> gtk::Button {
    let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mark.add_css_class("check");
    mark.set_valign(gtk::Align::Center);
    let text = gtk::Label::builder().label(&item.text).xalign(0.0).wrap(true).hexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.append(&mark);
    content.append(&text);
    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("item");
    if item.done {
        button.add_css_class("done");
    }
    let (note, item) = (note.clone(), item.clone());
    button.connect_clicked(move |_| {
        if let Err(err) = toggle(&note, &item) {
            eprintln!("notifyd: ticking {}: {err}", note.path.display());
        }
    });
    button
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "---\ntags: [plan]\n- [ ] not an item\n---\n# Kith\n\nThe plan.\n\n- [x] Spec the **assistant**\n- [ ] Write [[autark-ai|the daemon]]\n```\n- [ ] in code\n```\n  * [X] Pick a `name`\n";

    #[test]
    fn reads_the_checklist_and_skips_front_matter_and_code() {
        let note = parse(PathBuf::from("Kith.md"), "Kith", TEXT);
        let items: Vec<(&str, bool, usize)> = note.items.iter().map(|item| (item.text.as_str(), item.done, item.line)).collect();
        assert_eq!(items, vec![("Spec the assistant", true, 8), ("Write the daemon", false, 9), ("Pick a name", true, 13)]);
        assert_eq!(note.lines, vec!["The plan."]);
        assert_eq!(note.summary(), "2 of 3 done  ·  next: Write the daemon");
    }

    #[test]
    fn summary_without_a_checklist() {
        let note = parse(PathBuf::from("Ideas.md"), "Ideas", "# Ideas\nA calmer bar.\n");
        assert_eq!(note.summary(), "A calmer bar.");
    }

    #[test]
    fn toggles_only_the_same_item() {
        let dir = std::env::temp_dir().join(format!("notifyd-note-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Kith.md");
        std::fs::write(&path, TEXT).unwrap();
        let note = parse(path.clone(), "Kith", TEXT);
        toggle(&note, &note.items[1]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("- [x] Write [[autark-ai|the daemon]]\n"));
        toggle(&note, &note.items[0]).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("- [ ] Spec the **assistant**\n"));
        // A stale item (the file changed since) is left alone.
        let stale = Item { text: "Something else".to_owned(), done: false, line: 9 };
        toggle(&note, &stale).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("- [x] Write"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
