//! `autark-notes <file>`: one plain-text file in a window of its own, with no vault. For config files, scripts
//! and anything else that isn't a note, so nothing is autosaved or restyled: Ctrl+S writes it, and closing
//! with unsaved changes asks first.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{glib, pango};

struct File {
    window: gtk::ApplicationWindow,
    buffer: gtk::TextBuffer,
    view: gtk::TextView,
    path: PathBuf,
    name: String,
    dirty: Cell<bool>,
    /// The find bar's matches, as character ranges.
    matches: RefCell<Vec<(i32, i32)>>,
}

/// Opens `path`, which exists and is a file. Text only: a file that isn't UTF-8 is left alone.
pub fn open(app: &gtk::Application, path: &Path) -> Option<gtk::ApplicationWindow> {
    let text = match std::fs::read(path).map_err(|err| err.to_string()).and_then(|bytes| String::from_utf8(bytes).map_err(|_| "it isn't UTF-8 text".to_owned())) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("notes: {}: {err}", path.display());
            return None;
        }
    };
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let window = gtk::ApplicationWindow::builder().application(app).default_width(820).default_height(640).build();
    window.add_css_class("notes");
    window.add_css_class("file");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(&text);
    buffer.set_enable_undo(true);
    let view = gtk::TextView::builder().buffer(&buffer).monospace(true).wrap_mode(gtk::WrapMode::None).vexpand(true).build();
    view.add_css_class("code");
    let scroller = gtk::ScrolledWindow::builder().vexpand(true).child(&view).build();

    let controls = gtk::WindowControls::new(gtk::PackType::Start);
    controls.set_decoration_layout(Some("close,minimize,maximize:"));
    let title = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
    title.add_css_class("path");
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bar.add_css_class("toolbar");
    bar.append(&controls);
    bar.append(&title);

    let find = gtk::SearchEntry::builder().placeholder_text("Find").hexpand(true).build();
    let replace = gtk::Entry::builder().placeholder_text("Replace with").hexpand(true).build();
    let previous = gtk::Button::from_icon_name("go-up-symbolic");
    let next = gtk::Button::from_icon_name("go-down-symbolic");
    let replace_one = gtk::Button::with_label("Replace");
    let replace_all = gtk::Button::with_label("All");
    let close_find = gtk::Button::from_icon_name("window-close-symbolic");
    let find_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    find_bar.add_css_class("find");
    for widget in [find.upcast_ref::<gtk::Widget>(), replace.upcast_ref(), previous.upcast_ref(), next.upcast_ref(), replace_one.upcast_ref(), replace_all.upcast_ref(), close_find.upcast_ref()] {
        find_bar.append(widget);
    }
    let revealer = gtk::Revealer::builder().child(&find_bar).build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&gtk::WindowHandle::builder().child(&bar).build());
    content.append(&revealer);
    content.append(&scroller);
    window.set_child(Some(&content));

    let file = Rc::new(File { window: window.clone(), buffer, view, path: path.to_owned(), name, dirty: Cell::new(false), matches: RefCell::default() });
    let retitle = {
        let file = file.clone();
        let title = title.clone();
        move || {
            let shown = format!("{}{}", crate::window::display(&file.path), if file.dirty.get() { "  •" } else { "" });
            title.set_text(&shown);
            file.window.set_title(Some(&format!("{}{}", if file.dirty.get() { "• " } else { "" }, file.name)));
        }
    };
    retitle();

    let changed = {
        let file = file.clone();
        let retitle = retitle.clone();
        move |_: &gtk::TextBuffer| {
            file.dirty.set(true);
            retitle();
        }
    };
    file.buffer.connect_changed(changed);

    let save = {
        let file = file.clone();
        let retitle = retitle.clone();
        move || -> bool {
            let text = file.buffer.text(&file.buffer.start_iter(), &file.buffer.end_iter(), true);
            match write(&file.path, &text) {
                Ok(()) => {
                    file.dirty.set(false);
                    retitle();
                    true
                }
                Err(err) => {
                    eprintln!("notes: saving {}: {err}", file.path.display());
                    let dialog = gtk::AlertDialog::builder().message("Couldn't save").detail(format!("{}: {err}", file.path.display())).build();
                    dialog.show(Some(&file.window));
                    false
                }
            }
        }
    };

    // Ctrl+S saves; Ctrl+F and Ctrl+H open the find bar, Escape closes it.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let save = save.clone();
        let revealer = revealer.clone();
        let find = find.clone();
        let replace = replace.clone();
        let file = file.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            match (ctrl, key) {
                (true, gtk::gdk::Key::s) => {
                    save();
                    glib::Propagation::Stop
                }
                (true, gtk::gdk::Key::f | gtk::gdk::Key::h) => {
                    revealer.set_reveal_child(true);
                    if let Some((start, end)) = file.buffer.selection_bounds() {
                        find.set_text(&file.buffer.text(&start, &end, false));
                    }
                    if key == gtk::gdk::Key::h { replace.grab_focus() } else { find.grab_focus() };
                    glib::Propagation::Stop
                }
                (false, gtk::gdk::Key::Escape) if revealer.reveals_child() => {
                    revealer.set_reveal_child(false);
                    file.view.grab_focus();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
    }
    window.add_controller(keys);

    {
        let file = file.clone();
        let find2 = find.clone();
        find.connect_search_changed(move |_| file.search(&find2.text(), 0));
    }
    {
        let file = file.clone();
        let find = find.clone();
        next.connect_clicked(move |_| file.step(&find.text(), 1));
    }
    {
        let file = file.clone();
        let find = find.clone();
        previous.connect_clicked(move |_| file.step(&find.text(), -1));
    }
    {
        let file = file.clone();
        let find = find.clone();
        find.connect_activate({
            let find = find.clone();
            move |_| file.step(&find.text(), 1)
        });
    }
    {
        let file = file.clone();
        let find = find.clone();
        let replace = replace.clone();
        replace_one.connect_clicked(move |_| file.replace_selected(&find.text(), &replace.text()));
    }
    {
        let file = file.clone();
        let find = find.clone();
        let replace = replace.clone();
        replace_all.connect_clicked(move |_| file.replace_all(&find.text(), &replace.text()));
    }
    {
        let revealer = revealer.clone();
        let file = file.clone();
        close_find.connect_clicked(move |_| {
            revealer.set_reveal_child(false);
            file.view.grab_focus();
        });
    }

    // Closing with unsaved changes: save, discard or stay.
    {
        let file = file.clone();
        let save = save.clone();
        window.connect_close_request(move |window| {
            if !file.dirty.get() {
                return glib::Propagation::Proceed;
            }
            let dialog = gtk::AlertDialog::builder()
                .message(format!("Save changes to “{}”?", file.name))
                .detail("Your changes are lost if you don't save them.")
                .buttons(["Cancel", "Don't save", "Save"])
                .cancel_button(0)
                .default_button(2)
                .build();
            let window = window.clone();
            let file = file.clone();
            let save = save.clone();
            dialog.choose(Some(&window.clone()), gtk::gio::Cancellable::NONE, move |choice| {
                match choice {
                    Ok(2) if save() => file.dirty.set(false),
                    Ok(1) => file.dirty.set(false),
                    _ => return,
                }
                window.close();
            });
            glib::Propagation::Stop
        });
    }

    file.buffer.place_cursor(&file.buffer.start_iter());
    file.dirty.set(false);
    Some(window)
}

impl File {
    /// Finds every match of `query` (ignoring case) and selects the first at or after `from`.
    fn search(&self, query: &str, from: i32) {
        let mut matches = Vec::new();
        if !query.is_empty() {
            let mut at = self.buffer.start_iter();
            while let Some((start, end)) = at.forward_search(query, gtk::TextSearchFlags::CASE_INSENSITIVE, None) {
                matches.push((start.offset(), end.offset()));
                at = end;
            }
        }
        let first = matches.iter().position(|(start, _)| *start >= from).or((!matches.is_empty()).then_some(0));
        *self.matches.borrow_mut() = matches;
        if let Some(index) = first {
            self.select(index);
        }
    }

    fn select(&self, index: usize) {
        let Some((start, end)) = self.matches.borrow().get(index).copied() else { return };
        let (start, end) = (self.buffer.iter_at_offset(start), self.buffer.iter_at_offset(end));
        self.buffer.select_range(&start, &end);
        self.view.scroll_to_iter(&mut start.clone(), 0.2, false, 0.0, 0.0);
    }

    /// The next match (`1`) or the previous (`-1`) from the cursor, wrapping round.
    fn step(&self, query: &str, direction: i32) {
        if self.matches.borrow().is_empty() {
            self.search(query, self.cursor());
            return;
        }
        let cursor = self.cursor();
        let matches = self.matches.borrow().clone();
        let index = if direction > 0 {
            matches.iter().position(|(start, _)| *start > self.selection_start()).unwrap_or(0)
        } else {
            matches.iter().rposition(|(start, _)| *start < self.selection_start().min(cursor)).unwrap_or(matches.len() - 1)
        };
        self.select(index);
    }

    fn cursor(&self) -> i32 {
        self.buffer.iter_at_mark(&self.buffer.get_insert()).offset()
    }

    fn selection_start(&self) -> i32 {
        self.buffer.selection_bounds().map_or_else(|| self.cursor(), |(start, _)| start.offset())
    }

    fn replace_selected(&self, query: &str, with: &str) {
        if let Some((mut start, mut end)) = self.buffer.selection_bounds()
            && self.buffer.text(&start, &end, false).to_lowercase() == query.to_lowercase()
        {
            let at = start.offset();
            self.buffer.delete(&mut start, &mut end);
            self.buffer.insert(&mut self.buffer.iter_at_offset(at), with);
            self.search(query, at + with.chars().count() as i32);
        } else {
            self.step(query, 1);
        }
    }

    fn replace_all(&self, query: &str, with: &str) {
        self.search(query, 0);
        let matches = std::mem::take(&mut *self.matches.borrow_mut());
        if matches.is_empty() {
            return;
        }
        // One undo step for the lot, and from the end so earlier offsets hold.
        self.buffer.begin_user_action();
        for (start, end) in matches.into_iter().rev() {
            self.buffer.delete(&mut self.buffer.iter_at_offset(start), &mut self.buffer.iter_at_offset(end));
            self.buffer.insert(&mut self.buffer.iter_at_offset(start), with);
        }
        self.buffer.end_user_action();
    }
}

/// Writes through a hidden temporary file and a rename, keeping the file's permissions (a script stays
/// executable, a private file private) and writing through a symlink to the file it points at.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let path = std::fs::canonicalize(path)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.saving"));
    std::fs::write(&temporary, text)?;
    let result = std::fs::set_permissions(&temporary, std::fs::metadata(&path)?.permissions()).and_then(|()| std::fs::rename(&temporary, &path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
