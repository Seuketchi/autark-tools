//! The archive window: what's inside, with a size and date for each, and Extract All beside it. Select some of it
//! first and Extract Selected asks where they go. Drop another archive on the window to look in that.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};

use crate::archive::{self, Entry};

/// A row's text: path, size, whether a folder, and date, as the list model holds them.
const SEPARATOR: char = '\u{1}';

pub struct Window {
    pub window: gtk::ApplicationWindow,
    title: gtk::Label,
    info: gtk::Label,
    list: gtk::ListView,
    model: gtk::StringList,
    selection: gtk::MultiSelection,
    empty: gtk::Box,
    status: gtk::Label,
    show: gtk::Button,
    selected: gtk::Button,
    all: gtk::Button,
    archive: RefCell<Option<PathBuf>>,
    entries: RefCell<Vec<Entry>>,
    busy: Cell<bool>,
    /// Where the last extraction put things, for Show.
    last: RefCell<Option<PathBuf>>,
}

/// `1536` as "1.5 KB".
fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// A path with the home folder shown as `~`.
fn display(path: &Path) -> String {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    path.strip_prefix(&home).map_or_else(|_| path.display().to_string(), |rest| format!("~/{}", rest.display()))
}


impl Window {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Archive").default_width(780).default_height(560).build();
        window.add_css_class("archive");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let controls = gtk::WindowControls::new(gtk::PackType::Start);
        controls.set_decoration_layout(Some("close,minimize,maximize:"));
        let title = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
        title.add_css_class("title");
        let info = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        info.add_css_class("info");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top.add_css_class("top");
        top.append(&controls);
        top.append(&title);
        top.append(&info);
        let handle = gtk::WindowHandle::builder().child(&top).build();

        let model = gtk::StringList::new(&[]);
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else { return };
            let icon = gtk::Image::new();
            icon.add_css_class("kind");
            let path = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Middle).build();
            let size = gtk::Label::builder().xalign(1.0).width_chars(9).build();
            size.add_css_class("size");
            let date = gtk::Label::builder().xalign(0.0).width_chars(13).build();
            date.add_css_class("size");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            line.add_css_class("entry");
            for widget in [icon.upcast_ref::<gtk::Widget>(), path.upcast_ref(), size.upcast_ref(), date.upcast_ref()] {
                line.append(widget);
            }
            item.set_child(Some(&line));
        });
        factory.connect_bind(|_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else { return };
            let (Some(text), Some(line)) = (item.item().and_downcast::<gtk::StringObject>(), item.child()) else { return };
            let parts: Vec<String> = text.string().split(SEPARATOR).map(str::to_owned).collect();
            let [path, size, dir, date] = parts.as_slice() else { return };
            let mut children = std::iter::successors(line.first_child(), |child| child.next_sibling());
            if let Some(icon) = children.next().and_downcast::<gtk::Image>() {
                icon.set_icon_name(Some(if dir == "1" { "folder-symbolic" } else { "text-x-generic-symbolic" }));
            }
            if let Some(label) = children.next().and_downcast::<gtk::Label>() {
                label.set_text(path.trim_end_matches('/'));
            }
            if let Some(label) = children.next().and_downcast::<gtk::Label>() {
                label.set_text(&if dir == "1" { String::new() } else { size_text(size.parse().unwrap_or(0)) });
            }
            if let Some(label) = children.next().and_downcast::<gtk::Label>() {
                label.set_text(date);
            }
        });
        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        list.add_css_class("contents");
        list.set_show_separators(false);
        list.set_visible(false);
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&list).build();

        let open = gtk::Button::with_label("Open an archive");
        open.add_css_class("open");
        open.set_action_name(Some("win.open"));
        let hint = gtk::Label::new(Some("Zip, tar, 7z and more. Or drop one here."));
        hint.add_css_class("hint");
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 14);
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        empty.set_vexpand(true);
        empty.append(&gtk::Image::builder().icon_name("package-x-generic-symbolic").pixel_size(56).css_classes(["glyph"]).build());
        empty.append(&open);
        empty.append(&hint);

        let status = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
        status.add_css_class("status");
        let show = gtk::Button::with_label("Show");
        show.add_css_class("plain");
        show.set_visible(false);
        let selected = gtk::Button::with_label("Extract Selected…");
        selected.add_css_class("plain");
        selected.set_sensitive(false);
        let all = gtk::Button::with_label("Extract All");
        all.add_css_class("primary");
        all.set_sensitive(false);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        bottom.add_css_class("bottom");
        for widget in [status.upcast_ref::<gtk::Widget>(), show.upcast_ref(), selected.upcast_ref(), all.upcast_ref()] {
            bottom.append(widget);
        }

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&handle);
        content.append(&empty);
        content.append(&scroller);
        content.append(&bottom);
        window.set_child(Some(&content));

        let me = Rc::new(Self {
            window,
            title,
            info,
            list,
            model,
            selection,
            empty,
            status,
            show,
            selected,
            all,
            archive: RefCell::default(),
            entries: RefCell::default(),
            busy: Cell::new(false),
            last: RefCell::default(),
        });
        me.connect();
        me
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let on = |button: &gtk::Button, act: fn(&Rc<Window>)| {
            let weak = weak.clone();
            button.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    act(&me);
                }
            });
        };
        on(&self.all, |me| me.extract_all());
        on(&self.selected, |me| me.extract_selected());
        on(&self.show, |me| me.show_result());

        self.selection.connect_selection_changed({
            let weak = weak.clone();
            move |_, _, _| {
                if let Some(me) = weak.upgrade() {
                    me.sync();
                }
            }
        });
        // Double-clicking a row selects just it and, for a file, takes it out.
        self.list.connect_activate({
            let weak = weak.clone();
            move |_, position| {
                if let Some(me) = weak.upgrade() {
                    me.selection.unselect_all();
                    me.selection.select_item(position, true);
                    me.extract_selected();
                }
            }
        });

        let open = gio::SimpleAction::new("open", None);
        open.connect_activate({
            let weak = weak.clone();
            move |_, _| {
                if let Some(me) = weak.upgrade() {
                    me.choose_archive();
                }
            }
        });
        self.window.add_action(&open);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.open", &["<Control>o"]);
        }

        let drop = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        drop.connect_drop({
            let weak = weak.clone();
            move |_, value, _, _| match (weak.upgrade(), value.get::<gio::File>().ok().and_then(|file| file.path())) {
                (Some(me), Some(path)) => {
                    me.open(&path);
                    true
                }
                _ => false,
            }
        });
        self.window.add_controller(drop);

        // The window's handlers hold this only weakly, so something keeps it until the window closes.
        let keep = self.clone();
        self.window.connect_close_request(move |_| {
            let _ = &keep;
            glib::Propagation::Proceed
        });
    }

    pub fn fail(&self, message: &str) {
        self.status.set_text(message);
        self.status.add_css_class("error");
    }

    fn say(&self, message: &str) {
        self.status.set_text(message);
        self.status.remove_css_class("error");
    }

    /// Looks inside `path`.
    pub fn open(self: &Rc<Self>, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        self.title.set_text(&name);
        self.window.set_title(Some(&name));
        self.info.set_text("");
        self.say("Reading…");
        self.busy.set(true);
        self.sync();
        let (me, target) = (self.clone(), path.clone());
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking({
                let target = target.clone();
                move || archive::list(&target)
            })
            .await;
            me.busy.set(false);
            match result {
                Ok(Ok(entries)) => me.show_entries(target, entries),
                Ok(Err(error)) => {
                    me.fail(&format!("Couldn’t read {name}: {error}"));
                    me.sync();
                }
                Err(_) => me.fail("Reading the archive failed."),
            }
        });
    }

    fn show_entries(&self, path: PathBuf, entries: Vec<Entry>) {
        let rows: Vec<String> = entries.iter().map(|entry| [entry.path.as_str(), &entry.size.to_string(), if entry.dir { "1" } else { "0" }, &entry.modified].join(&SEPARATOR.to_string())).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        self.model.splice(0, self.model.n_items(), &rows);
        let files = entries.iter().filter(|entry| !entry.dir).count();
        let total: u64 = entries.iter().map(|entry| entry.size).sum();
        self.info.set_text(&format!("{files} {} · {} unpacked", if files == 1 { "file" } else { "files" }, size_text(total)));
        self.empty.set_visible(false);
        self.list.set_visible(true);
        self.show.set_visible(false);
        self.say(&format!("In {}", display(path.parent().unwrap_or_else(|| Path::new(".")))));
        *self.archive.borrow_mut() = Some(path);
        *self.entries.borrow_mut() = entries;
        self.sync();
    }

    /// Which buttons can be pressed.
    fn sync(&self) {
        let ready = self.archive.borrow().is_some() && !self.busy.get();
        self.all.set_sensitive(ready && !self.entries.borrow().is_empty());
        self.selected.set_sensitive(ready && self.selection.selection().size() > 0);
    }

    fn choose_archive(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Open an archive").build();
        let me = Rc::downgrade(self);
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            if let (Some(me), Ok(file)) = (me.upgrade(), result)
                && let Some(path) = file.path()
            {
                me.open(&path);
            }
        });
    }

    fn extract_all(self: &Rc<Self>) {
        let Some(parent) = self.archive.borrow().as_ref().and_then(|path| path.parent().map(Path::to_path_buf)) else { return };
        self.extract(parent, Vec::new());
    }

    fn extract_selected(self: &Rc<Self>) {
        let picked = self.picked();
        if picked.is_empty() {
            return;
        }
        let start = self.archive.borrow().as_ref().and_then(|path| path.parent().map(Path::to_path_buf));
        let dialog = gtk::FileDialog::builder().title("Extract to").build();
        if let Some(start) = start {
            dialog.set_initial_folder(Some(&gio::File::for_path(start)));
        }
        let me = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            if let (Some(me), Ok(folder)) = (me.upgrade(), result)
                && let Some(path) = folder.path()
            {
                me.extract(path, picked.clone());
            }
        });
    }

    /// The paths of what's selected.
    fn picked(&self) -> Vec<String> {
        let selected = self.selection.selection();
        (0..selected.size() as u32)
            .filter_map(|n| self.model.string(selected.nth(n)))
            .filter_map(|text| text.split(SEPARATOR).next().map(str::to_owned))
            .collect()
    }

    /// Takes `picked` (everything, when empty) out into `dest`, off the main thread.
    fn extract(self: &Rc<Self>, dest: PathBuf, picked: Vec<String>) {
        let Some(path) = self.archive.borrow().clone() else { return };
        let entries = self.entries.borrow().clone();
        self.busy.set(true);
        self.say("Extracting…");
        self.show.set_visible(false);
        self.sync();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || archive::extract(&path, &dest, &picked, &entries)).await;
            me.busy.set(false);
            match result {
                Ok(Ok(made)) => {
                    let first = made.first().cloned();
                    me.say(&match (made.len(), first.as_deref()) {
                        (1, Some(one)) => format!("Extracted to {}", display(one)),
                        (count, _) => format!("Extracted {count} items to {}", display(made.first().and_then(|path| path.parent()).unwrap_or_else(|| Path::new(".")))),
                    });
                    *me.last.borrow_mut() = first;
                    me.show.set_visible(true);
                }
                Ok(Err(error)) => me.fail(&format!("Couldn’t extract: {error}")),
                Err(_) => me.fail("Extracting failed."),
            }
            me.sync();
        });
    }

    /// Shows what was just extracted in Files.
    fn show_result(&self) {
        let Some(path) = self.last.borrow().clone() else { return };
        let target = if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
        if let Err(error) = std::process::Command::new("autark-files").arg(&target).spawn() {
            self.fail(&format!("Couldn’t open Files: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_paths_read_plainly() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(1536), "1.5 KB");
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(display(&PathBuf::from(format!("{home}/Downloads/a.zip"))), "~/Downloads/a.zip");
        assert_eq!(display(Path::new("/tmp/a.zip")), "/tmp/a.zip");
    }
}
