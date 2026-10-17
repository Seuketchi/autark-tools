//! The quick switcher (Ctrl+O): type part of a title, press Enter to open it, or to create the
//! note when nothing matches.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::vault::Vault;

type OnPick = RefCell<Option<Box<dyn Fn(&Pick)>>>;

pub enum Pick {
    Open(String),
    Create(String),
}

pub struct Switcher {
    pub widget: gtk::Box,
    entry: gtk::Entry,
    results: gtk::Box,
    picks: RefCell<Vec<Pick>>,
    selected: Cell<usize>,
    /// The note list the results come from, refreshed each time it opens.
    titles: RefCell<Vec<(String, String, std::time::SystemTime)>>,
    on_pick: OnPick,
}

const SHOWN: usize = 8;

impl Switcher {
    pub fn new() -> Rc<Self> {
        let entry = gtk::Entry::builder().placeholder_text("Find or create a note").build();
        let results = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.add_css_class("switcher");
        widget.set_halign(gtk::Align::Center);
        widget.set_valign(gtk::Align::Start);
        widget.set_size_request(520, -1);
        widget.append(&entry);
        widget.append(&results);
        widget.set_visible(false);
        let switcher = Rc::new(Self {
            widget,
            entry,
            results,
            picks: RefCell::default(),
            selected: Cell::new(0),
            titles: RefCell::default(),
            on_pick: RefCell::default(),
        });

        let weak = Rc::downgrade(&switcher);
        switcher.entry.connect_changed(move |_| {
            if let Some(switcher) = weak.upgrade() {
                switcher.update();
            }
        });
        let weak = Rc::downgrade(&switcher);
        switcher.entry.connect_activate(move |_| {
            if let Some(switcher) = weak.upgrade() {
                switcher.pick(switcher.selected.get());
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&switcher);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(switcher) = weak.upgrade() else { return glib::Propagation::Proceed };
            let count = switcher.picks.borrow().len();
            match key {
                gdk::Key::Escape => switcher.hide(),
                gdk::Key::Down if count > 0 => switcher.select((switcher.selected.get() + 1) % count),
                gdk::Key::Up if count > 0 => switcher.select((switcher.selected.get() + count - 1) % count),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        switcher.widget.add_controller(keys);
        switcher
    }

    pub fn connect_pick(&self, f: impl Fn(&Pick) + 'static) {
        *self.on_pick.borrow_mut() = Some(Box::new(f));
    }

    pub fn show(self: &Rc<Self>, vault: &Vault) {
        self.titles.replace(vault.notes.iter().map(|note| (note.path.clone(), note.title.clone(), note.modified)).collect());
        self.entry.set_text("");
        self.update();
        self.widget.set_visible(true);
        self.entry.grab_focus();
    }

    pub fn hide(&self) {
        self.widget.set_visible(false);
    }

    pub fn is_shown(&self) -> bool {
        self.widget.is_visible()
    }

    fn update(self: &Rc<Self>) {
        let query = self.entry.text().trim().to_owned();
        let titles = self.titles.borrow();
        let mut ranked: Vec<(u32, &(String, String, std::time::SystemTime))> =
            titles.iter().filter_map(|entry| score(&entry.1, &query).map(|score| (score, entry))).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.2.cmp(&a.1.2)));
        let mut picks: Vec<Pick> = ranked.iter().take(SHOWN).map(|(_, (path, ..))| Pick::Open(path.clone())).collect();
        let exact = titles.iter().any(|(_, title, _)| title.eq_ignore_ascii_case(&query));
        if !query.is_empty() && !exact {
            picks.push(Pick::Create(query.clone()));
        }

        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        for (i, pick) in picks.iter().enumerate() {
            let (text, detail) = match pick {
                Pick::Open(path) => {
                    let title = titles.iter().find(|(p, ..)| p == path).map(|(_, title, _)| title.clone()).unwrap_or_default();
                    (title, path.rsplit_once('/').map(|(folder, _)| folder.to_owned()).unwrap_or_default())
                }
                Pick::Create(title) => (format!("Create “{title}”"), String::new()),
            };
            let label = gtk::Label::builder().label(&text).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            let folder = gtk::Label::new(Some(&detail));
            folder.add_css_class("folder-name");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&label);
            row.append(&folder);
            let button = gtk::Button::builder().child(&row).build();
            button.add_css_class("result");
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(switcher) = weak.upgrade() {
                    switcher.pick(i);
                }
            });
            self.results.append(&button);
        }
        drop(titles);
        self.picks.replace(picks);
        self.select(0);
    }

    fn select(&self, index: usize) {
        self.selected.set(index);
        let mut child = self.results.first_child();
        let mut i = 0;
        while let Some(widget) = child {
            if i == index {
                widget.add_css_class("selected");
            } else {
                widget.remove_css_class("selected");
            }
            child = widget.next_sibling();
            i += 1;
        }
    }

    fn pick(&self, index: usize) {
        let picks = self.picks.borrow();
        let Some(pick) = picks.get(index) else { return };
        self.hide();
        if let Some(on_pick) = &*self.on_pick.borrow() {
            on_pick(pick);
        }
    }
}

/// How well `title` matches `query`: a prefix beats a word start beats a substring beats the
/// letters in order. `None` when it doesn't match; every title matches an empty query.
fn score(title: &str, query: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let (title, query) = (title.to_lowercase(), query.to_lowercase());
    if title.starts_with(&query) {
        return Some(400);
    }
    if title.split_whitespace().any(|word| word.starts_with(&query)) {
        return Some(300);
    }
    if title.contains(&query) {
        return Some(200);
    }
    let mut letters = title.chars();
    query.chars().filter(|c| !c.is_whitespace()).all(|c| letters.any(|t| t == c)).then_some(100)
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn ranking() {
        assert!(score("Autark roadmap", "aut") > score("Build log", "log"));
        assert!(score("Autark roadmap", "road") > score("Railroad", "road"));
        assert_eq!(score("Autark roadmap", "admp"), Some(100));
        assert_eq!(score("Kith", "xyz"), None);
    }
}
