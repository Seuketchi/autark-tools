//! The Notes window: folders and tags, the note list, and the editor with its backlinks.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};

use crate::editor::Editor;
use crate::graph::Graph;
use crate::state::State;
use crate::switcher::{Pick, Switcher};
use crate::vault::{Note, Vault};

/// A window action: what it runs when its shortcut is pressed.
type Action = fn(&Rc<Notes>);

pub struct Notes {
    pub window: gtk::ApplicationWindow,
    vault: RefCell<Vault>,
    state: RefCell<State>,
    /// `None` shows every note.
    folder: RefCell<Option<String>>,
    tag: RefCell<Option<String>>,
    current: RefCell<Option<String>>,
    /// A note made with New and not yet written in; it's removed if left empty.
    fresh: RefCell<Option<String>>,
    /// The editor has text the file doesn't.
    dirty: Cell<bool>,
    save_generation: Cell<u64>,
    rescan_pending: Cell<bool>,
    monitors: RefCell<Vec<gio::FileMonitor>>,
    vault_label: gtk::Label,
    folders: gtk::Box,
    tags: gtk::FlowBox,
    search: gtk::SearchEntry,
    list: gtk::Box,
    path: gtk::Label,
    stack: gtk::Stack,
    pub editor: Rc<Editor>,
    backlinks_title: gtk::Label,
    backlinks: gtk::FlowBox,
    graph: Rc<Graph>,
    note_view: gtk::ToggleButton,
    graph_view: gtk::ToggleButton,
    switcher: Rc<Switcher>,
    /// The switcher's pick opens in a new tab.
    switch_to_new_tab: Cell<bool>,
    /// Open notes, one per tab; the strip shows once there are two.
    tabs: RefCell<Vec<String>>,
    tab: Cell<usize>,
    tab_strip: gtk::Box,
    find: gtk::Revealer,
    find_entry: gtk::SearchEntry,
    find_count: gtk::Label,
    replace_row: gtk::Box,
    replace_entry: gtk::Entry,
    find_index: Cell<usize>,
}

impl Notes {
    pub fn new(app: &gtk::Application, root: &Path) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Notes").default_width(1240).default_height(760).build();
        window.add_css_class("notes");
        // The window controls sit in the sidebar, as in the design.
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let controls = gtk::WindowControls::new(gtk::PackType::Start);
        controls.set_decoration_layout(Some("close,minimize,maximize:"));
        controls.set_halign(gtk::Align::Start);
        let vault_label = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Start).build();
        vault_label.add_css_class("heading");
        let folders = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let tags_heading = gtk::Label::builder().label("TAGS").xalign(0.0).build();
        tags_heading.add_css_class("heading");
        let tags = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(6).row_spacing(6).homogeneous(false).build();
        tags.add_css_class("tags");
        let sidebar_content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        sidebar_content.append(&vault_label);
        sidebar_content.append(&folders);
        sidebar_content.append(&tags_heading);
        sidebar_content.append(&tags);
        let sidebar_scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&sidebar_content).build();
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar");
        sidebar.set_size_request(210, -1);
        sidebar.append(&gtk::WindowHandle::builder().child(&controls).build());
        sidebar.append(&sidebar_scroller);

        let search = gtk::SearchEntry::builder().placeholder_text("Search all notes").hexpand(true).build();
        let new = gtk::Button::from_icon_name("list-add-symbolic");
        new.set_tooltip_text(Some("New note (Ctrl+N)"));
        new.add_css_class("new");
        new.set_action_name(Some("win.new-note"));
        let list_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        list_header.add_css_class("list-header");
        list_header.append(&search);
        list_header.append(&new);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.add_css_class("note-list");
        let list_scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&list).build();
        let list_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list_pane.add_css_class("list-pane");
        list_pane.set_size_request(300, -1);
        list_pane.append(&gtk::WindowHandle::builder().child(&list_header).build());
        list_pane.append(&list_scroller);

        let path = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Start).build();
        path.add_css_class("path");
        let checklist = gtk::Button::from_icon_name("view-list-bullet-symbolic");
        checklist.set_tooltip_text(Some("Checklist"));
        checklist.add_css_class("tool");
        let link = gtk::Button::with_label("[[ ]]");
        link.set_tooltip_text(Some("Insert link"));
        link.add_css_class("tool");
        link.add_css_class("link-tool");
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        toolbar.add_css_class("toolbar");
        toolbar.append(&path);
        toolbar.append(&checklist);
        toolbar.append(&link);
        let note_view = gtk::ToggleButton::builder().label("Note").active(true).build();
        let graph_view = gtk::ToggleButton::builder().label("Graph").group(&note_view).build();
        let views = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        views.add_css_class("views");
        views.append(&note_view);
        views.append(&graph_view);
        toolbar.append(&views);

        let editor = Editor::new();
        let backlinks_title = gtk::Label::builder().xalign(0.0).build();
        backlinks_title.add_css_class("heading");
        let backlinks = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(8).row_spacing(8).build();
        let footer = gtk::Box::new(gtk::Orientation::Vertical, 8);
        footer.add_css_class("backlinks");
        footer.append(&backlinks_title);
        footer.append(&backlinks);
        let find_entry = gtk::SearchEntry::builder().placeholder_text("Find").hexpand(true).build();
        let find_count = gtk::Label::new(None);
        find_count.add_css_class("count");
        let previous = gtk::Button::from_icon_name("go-up-symbolic");
        previous.set_tooltip_text(Some("Previous (Shift+Enter)"));
        let next = gtk::Button::from_icon_name("go-down-symbolic");
        next.set_tooltip_text(Some("Next (Enter)"));
        let close_find = gtk::Button::from_icon_name("window-close-symbolic");
        close_find.set_tooltip_text(Some("Close (Esc)"));
        let find_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for widget in [find_entry.upcast_ref::<gtk::Widget>(), find_count.upcast_ref(), previous.upcast_ref(), next.upcast_ref(), close_find.upcast_ref()] {
            find_row.append(widget);
        }
        let replace_entry = gtk::Entry::builder().placeholder_text("Replace with").hexpand(true).build();
        let replace_one = gtk::Button::with_label("Replace");
        let replace_all = gtk::Button::with_label("All");
        let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        replace_row.append(&replace_entry);
        replace_row.append(&replace_one);
        replace_row.append(&replace_all);
        let find_bar = gtk::Box::new(gtk::Orientation::Vertical, 6);
        find_bar.add_css_class("find");
        find_bar.append(&find_row);
        find_bar.append(&replace_row);
        let find = gtk::Revealer::builder().child(&find_bar).reveal_child(false).build();

        let note_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        note_page.append(&find);
        note_page.append(&editor.widget);
        note_page.append(&footer);
        let empty = gtk::Label::new(Some("No note open. Press + to write one."));
        empty.add_css_class("empty");
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&note_page, Some("note"));
        stack.add_named(&empty, Some("empty"));
        let graph = Graph::new();
        stack.add_named(&graph.widget, Some("graph"));
        let tab_strip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab_strip.add_css_class("tab-strip");
        tab_strip.set_visible(false);

        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.add_css_class("main");
        main.set_hexpand(true);
        main.append(&gtk::WindowHandle::builder().child(&toolbar).build());
        main.append(&tab_strip);
        main.append(&stack);

        let root_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root_box.append(&sidebar);
        root_box.append(&list_pane);
        root_box.append(&main);
        let switcher = Switcher::new();
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&root_box));
        overlay.add_overlay(&switcher.widget);
        window.set_child(Some(&overlay));

        let notes = Rc::new(Self {
            window,
            vault: RefCell::new(Vault::scan(root)),
            state: RefCell::new(State::load(root)),
            folder: RefCell::default(),
            tag: RefCell::default(),
            current: RefCell::default(),
            fresh: RefCell::default(),
            dirty: Cell::new(false),
            save_generation: Cell::new(0),
            rescan_pending: Cell::new(false),
            monitors: RefCell::default(),
            vault_label,
            folders,
            tags,
            search,
            list,
            path,
            stack,
            editor,
            backlinks_title,
            backlinks,
            graph,
            note_view,
            graph_view,
            switcher,
            switch_to_new_tab: Cell::new(false),
            tabs: RefCell::default(),
            tab: Cell::new(0),
            tab_strip,
            find,
            find_entry,
            find_count,
            replace_row,
            replace_entry,
            find_index: Cell::new(0),
        });
        notes.connect_find(&previous, &next, &close_find, &replace_one, &replace_all);

        let weak = Rc::downgrade(&notes);
        notes.search.connect_search_changed(move |_| {
            if let Some(notes) = weak.upgrade() {
                notes.refresh_list();
            }
        });
        let weak = Rc::downgrade(&notes);
        checklist.connect_clicked(move |_| {
            if let Some(notes) = weak.upgrade() {
                notes.editor.toggle_checklist();
            }
        });
        let weak = Rc::downgrade(&notes);
        link.connect_clicked(move |_| {
            if let Some(notes) = weak.upgrade() {
                notes.editor.insert_link();
            }
        });
        let weak = Rc::downgrade(&notes);
        notes.editor.connect_edit(move || {
            if let Some(notes) = weak.upgrade() {
                notes.edited();
            }
        });
        let weak = Rc::downgrade(&notes);
        notes.editor.connect_link(move |target, new_tab| {
            if let Some(notes) = weak.upgrade() {
                notes.follow(&target, new_tab);
            }
        });
        let weak = Rc::downgrade(&notes);
        notes.graph.connect_open(move |path| {
            if let Some(notes) = weak.upgrade() {
                notes.open(&path);
            }
        });
        let weak = Rc::downgrade(&notes);
        notes.graph_view.connect_toggled(move |button| {
            if let Some(notes) = weak.upgrade() {
                notes.show_view(button.is_active());
            }
        });
        let weak = Rc::downgrade(&notes);
        notes.switcher.connect_pick(move |pick| {
            let Some(notes) = weak.upgrade() else { return };
            let new_tab = notes.switch_to_new_tab.replace(false);
            match pick {
                Pick::Open(path) => notes.show(path, new_tab),
                Pick::Create(title) => notes.follow(title, new_tab),
            }
        });
        let weak = Rc::downgrade(&notes);
        notes.editor.connect_rename(move |title| {
            if let Some(notes) = weak.upgrade() {
                notes.rename(&title);
            }
        });
        let weak = Rc::downgrade(&notes);
        notes.window.connect_close_request(move |_| {
            if let Some(notes) = weak.upgrade() {
                notes.save();
            }
            glib::Propagation::Proceed
        });
        notes.add_actions(app);
        notes.start();
        notes
    }

    fn add_actions(self: &Rc<Self>, app: &gtk::Application) {
        let actions: [(&str, &[&str], Action); 11] = [
            ("new-note", &["<Control>n"], |notes| notes.new_note()),
            ("save", &["<Control>s"], |notes| notes.save()),
            ("search", &["<Control><Shift>f"], |notes| {
                notes.search.grab_focus();
            }),
            ("open-vault", &["<Control><Shift>o"], |notes| notes.choose_vault()),
            ("quick-open", &["<Control>o"], |notes| notes.quick_open(false)),
            ("new-tab", &["<Control>t"], |notes| notes.quick_open(true)),
            ("close-tab", &["<Control>w"], |notes| notes.close_tab(notes.tab.get())),
            ("next-tab", &["<Control>Page_Down", "<Control>Tab"], |notes| notes.cycle_tab(1)),
            ("previous-tab", &["<Control>Page_Up", "<Control><Shift>Tab"], |notes| notes.cycle_tab(-1)),
            ("find", &["<Control>f"], |notes| notes.show_find(false)),
            ("replace", &["<Control>h"], |notes| notes.show_find(true)),
        ];
        for (name, accels, run) in actions {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(notes) = weak.upgrade() {
                    run(&notes);
                }
            });
            self.window.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }
    }

    /// Shows the vault as loaded: the remembered note, or the first one.
    fn start(self: &Rc<Self>) {
        self.vault_label.set_label(&display(&self.vault.borrow().root));
        self.watch();
        self.refresh_sidebar();
        let tabs: Vec<String> = self.state.borrow().tabs.iter().filter(|path| self.vault.borrow().get(path).is_some()).cloned().collect();
        self.tabs.replace(tabs);
        self.tab.set(0);
        let remembered = self.state.borrow().open.clone().filter(|path| self.vault.borrow().get(path).is_some());
        match remembered.or_else(|| self.shown().first().cloned()) {
            Some(path) => self.open(&path),
            None => {
                self.refresh_list();
                self.stack.set_visible_child_name("empty");
            }
        }
    }

    fn set_vault(self: &Rc<Self>, root: &Path) {
        self.save();
        crate::config::remember_vault(root);
        *self.vault.borrow_mut() = Vault::scan(root);
        *self.state.borrow_mut() = State::load(root);
        self.folder.replace(None);
        self.tag.replace(None);
        self.current.replace(None);
        self.search.set_text("");
        self.tabs.replace(Vec::new());
        self.start();
    }

    fn choose_vault(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Open a folder of notes").modal(true).build();
        dialog.set_initial_folder(Some(&gio::File::for_path(&self.vault.borrow().root)));
        let weak = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let (Some(notes), Ok(folder)) = (weak.upgrade(), result) else { return };
            if let Some(path) = folder.path() {
                notes.set_vault(&path);
            }
        });
    }

    /// The notes the list shows: pinned first, then newest first.
    fn shown(&self) -> Vec<String> {
        let vault = self.vault.borrow();
        let state = self.state.borrow();
        let folder = self.folder.borrow();
        let tag = self.tag.borrow();
        let query = self.search.text();
        let mut notes: Vec<&Note> = vault
            .notes
            .iter()
            .filter(|note| folder.as_ref().is_none_or(|folder| &note.folder == folder))
            .filter(|note| tag.as_ref().is_none_or(|tag| note.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))))
            .filter(|note| note.matches(&query))
            .collect();
        notes.sort_by_key(|note| (!state.is_pinned(&note.path), std::cmp::Reverse(note.modified)));
        notes.into_iter().map(|note| note.path.clone()).collect()
    }

    fn refresh_sidebar(self: &Rc<Self>) {
        while let Some(child) = self.folders.first_child() {
            self.folders.remove(&child);
        }
        let vault = self.vault.borrow();
        let current = self.folder.borrow().clone();
        let mut entries = vec![(None, "All notes".to_owned(), 0, vault.notes.len())];
        for folder in &vault.folders {
            let depth = folder.matches('/').count();
            let name = folder.rsplit('/').next().unwrap_or(folder).to_owned();
            let count = vault.notes.iter().filter(|note| &note.folder == folder).count();
            entries.push((Some(folder.clone()), name, depth, count));
        }
        for (folder, name, depth, count) in entries {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let icon = gtk::Image::from_icon_name("folder-symbolic");
            let label = gtk::Label::builder().label(&name).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            let count_label = gtk::Label::new(Some(&count.to_string()));
            count_label.add_css_class("count");
            row.append(&icon);
            row.append(&label);
            row.append(&count_label);
            let button = gtk::Button::builder().child(&row).build();
            button.add_css_class("folder");
            button.set_margin_start(depth as i32 * 14);
            if folder == current && self.tag.borrow().is_none() {
                button.add_css_class("current");
            }
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(notes) = weak.upgrade() else { return };
                notes.folder.replace(folder.clone());
                notes.tag.replace(None);
                notes.refresh_sidebar();
                notes.refresh_list();
            });
            self.folders.append(&button);
        }

        self.tags.remove_all();
        let tag = self.tag.borrow().clone();
        for (name, _) in vault.tags() {
            let button = gtk::Button::with_label(&format!("#{name}"));
            button.add_css_class("tag");
            if tag.as_ref().is_some_and(|tag| tag.eq_ignore_ascii_case(&name)) {
                button.add_css_class("current");
            }
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(notes) = weak.upgrade() else { return };
                let same = notes.tag.borrow().as_ref().is_some_and(|tag| tag.eq_ignore_ascii_case(&name));
                notes.tag.replace((!same).then(|| name.clone()));
                notes.folder.replace(None);
                notes.refresh_sidebar();
                notes.refresh_list();
            });
            self.tags.append(&button);
        }
    }

    fn refresh_list(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let shown = self.shown();
        if shown.is_empty() {
            let empty = gtk::Label::new(Some(if self.search.text().is_empty() { "No notes here yet" } else { "No notes match" }));
            empty.add_css_class("empty");
            self.list.append(&empty);
            return;
        }
        let vault = self.vault.borrow();
        let state = self.state.borrow();
        let current = self.current.borrow().clone();
        let mut group = None;
        for path in shown {
            let Some(note) = vault.get(&path) else { continue };
            let pinned = state.is_pinned(&path);
            if group != Some(pinned) {
                group = Some(pinned);
                let heading = gtk::Label::builder().label(if pinned { "PINNED" } else { "NOTES" }).xalign(0.0).build();
                heading.add_css_class("heading");
                self.list.append(&heading);
            }
            let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            if pinned {
                title_row.append(&gtk::Image::from_icon_name("view-pin-symbolic"));
            }
            let title = gtk::Label::builder().label(&note.title).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            title.add_css_class("note-title");
            title_row.append(&title);
            let date = gtk::Label::new(Some(&short_date(note.modified)));
            date.add_css_class("date");
            let preview = gtk::Label::builder().label(&note.preview).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            preview.add_css_class("preview");
            let sub = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            sub.append(&date);
            sub.append(&preview);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 3);
            content.append(&title_row);
            content.append(&sub);
            let button = gtk::Button::builder().child(&content).build();
            button.add_css_class("note");
            if current.as_deref() == Some(path.as_str()) {
                button.add_css_class("current");
            }
            let weak = Rc::downgrade(self);
            let open_path = path.clone();
            button.connect_clicked(move |_| {
                if let Some(notes) = weak.upgrade() {
                    notes.open(&open_path);
                }
            });
            let menu = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let weak = Rc::downgrade(self);
            let anchor = button.clone();
            menu.connect_pressed(move |_, _, _, _| {
                if let Some(notes) = weak.upgrade() {
                    notes.note_menu(&anchor, &path);
                }
            });
            button.add_controller(menu);
            self.list.append(&button);
        }
    }

    /// Pin and Move to Trash, on a right click.
    fn note_menu(self: &Rc<Self>, anchor: &gtk::Button, path: &str) {
        let pinned = self.state.borrow().is_pinned(path);
        let popover = gtk::Popover::new();
        popover.add_css_class("menu");
        popover.set_has_arrow(false);
        let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let pin = gtk::Button::with_label(if pinned { "Unpin" } else { "Pin" });
        let trash = gtk::Button::with_label("Move to Trash");
        items.append(&pin);
        items.append(&trash);
        popover.set_child(Some(&items));
        popover.set_parent(anchor);
        let (weak, path_for_pin, menu) = (Rc::downgrade(self), path.to_owned(), popover.clone());
        pin.connect_clicked(move |_| {
            menu.popdown();
            if let Some(notes) = weak.upgrade() {
                notes.toggle_pin(&path_for_pin);
            }
        });
        let (weak, path, menu) = (Rc::downgrade(self), path.to_owned(), popover.clone());
        trash.connect_clicked(move |_| {
            menu.popdown();
            if let Some(notes) = weak.upgrade() {
                notes.trash(&path);
            }
        });
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }

    pub fn toggle_pin(self: &Rc<Self>, path: &str) {
        {
            let mut state = self.state.borrow_mut();
            if state.is_pinned(path) {
                state.pinned.retain(|pinned| pinned != path);
            } else {
                state.pinned.push(path.to_owned());
            }
            state.save(&self.vault.borrow().root);
        }
        self.refresh_list();
    }

    fn trash(self: &Rc<Self>, path: &str) {
        if self.current.borrow().as_deref() == Some(path) {
            self.save();
        }
        let file = gio::File::for_path(self.vault.borrow().full_path(path));
        if let Err(err) = file.trash(gio::Cancellable::NONE) {
            eprintln!("notes: moving {path} to the trash: {err}");
            return;
        }
        self.vault.borrow_mut().notes.retain(|note| note.path != path);
        {
            let mut state = self.state.borrow_mut();
            state.pinned.retain(|pinned| pinned != path);
            state.save(&self.vault.borrow().root);
        }
        let was_open = self.current.borrow().as_deref() == Some(path);
        self.prune_tabs();
        if was_open {
            self.current.replace(None);
            let next = self.tabs.borrow().get(self.tab.get()).cloned().or_else(|| self.shown().first().cloned());
            match next {
                Some(next) => self.open(&next),
                None => self.stack.set_visible_child_name("empty"),
            }
        }
        self.refresh_sidebar();
        self.refresh_list();
    }

    pub fn open(self: &Rc<Self>, path: &str) {
        self.show(path, false);
    }

    /// Opens a note in the current tab, or a new one. A note already open in a tab switches
    /// to that tab instead.
    pub fn show(self: &Rc<Self>, path: &str, new_tab: bool) {
        self.save();
        self.drop_fresh(path);
        if self.vault.borrow().get(path).is_none() {
            return;
        }
        {
            let mut tabs = self.tabs.borrow_mut();
            if let Some(index) = tabs.iter().position(|tab| tab == path) {
                self.tab.set(index);
            } else if new_tab || tabs.is_empty() {
                tabs.push(path.to_owned());
                self.tab.set(tabs.len() - 1);
            } else {
                let index = self.tab.get().min(tabs.len() - 1);
                tabs[index] = path.to_owned();
                self.tab.set(index);
            }
        }
        let (title, text) = {
            let vault = self.vault.borrow();
            let Some(note) = vault.get(path) else { return };
            (note.title.clone(), note.text.clone())
        };
        self.current.replace(Some(path.to_owned()));
        {
            let mut state = self.state.borrow_mut();
            state.open = Some(path.to_owned());
            state.tabs = self.tabs.borrow().clone();
            state.save(&self.vault.borrow().root);
        }
        self.editor.load(&title, &text);
        self.path.set_label(&format!("{}/{path}", display(&self.vault.borrow().root)));
        self.note_view.set_active(true);
        self.stack.set_visible_child_name("note");
        self.refresh_backlinks();
        self.refresh_list();
        self.refresh_tabs();
        if self.find.reveals_child() {
            self.found(self.editor.match_from_cursor());
        }
    }

    fn refresh_tabs(self: &Rc<Self>) {
        while let Some(child) = self.tab_strip.first_child() {
            self.tab_strip.remove(&child);
        }
        let tabs = self.tabs.borrow();
        self.tab_strip.set_visible(tabs.len() > 1);
        let vault = self.vault.borrow();
        for (index, path) in tabs.iter().enumerate() {
            let title = vault.get(path).map(|note| note.title.clone()).unwrap_or_default();
            let label = gtk::Label::builder().label(&title).ellipsize(pango::EllipsizeMode::End).max_width_chars(18).build();
            let select = gtk::Button::builder().child(&label).build();
            let close = gtk::Button::from_icon_name("window-close-symbolic");
            close.add_css_class("close");
            close.set_tooltip_text(Some("Close tab (Ctrl+W)"));
            let tab = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            tab.add_css_class("tab");
            if index == self.tab.get() {
                tab.add_css_class("current");
            }
            tab.append(&select);
            tab.append(&close);
            let (weak, path) = (Rc::downgrade(self), path.clone());
            select.connect_clicked(move |_| {
                if let Some(notes) = weak.upgrade() {
                    notes.open(&path);
                }
            });
            let weak = Rc::downgrade(self);
            close.connect_clicked(move |_| {
                if let Some(notes) = weak.upgrade() {
                    notes.close_tab(index);
                }
            });
            self.tab_strip.append(&tab);
        }
    }

    fn close_tab(self: &Rc<Self>, index: usize) {
        if self.tabs.borrow().len() < 2 || index >= self.tabs.borrow().len() {
            return;
        }
        self.save();
        self.tabs.borrow_mut().remove(index);
        let current = self.tab.get();
        let next = if index < current { current - 1 } else { current.min(self.tabs.borrow().len() - 1) };
        self.tab.set(next);
        let path = self.tabs.borrow()[next].clone();
        self.open(&path);
    }

    fn cycle_tab(self: &Rc<Self>, step: isize) {
        let count = self.tabs.borrow().len();
        if count < 2 {
            return;
        }
        let next = (self.tab.get() as isize + step).rem_euclid(count as isize) as usize;
        let path = self.tabs.borrow()[next].clone();
        self.open(&path);
    }

    /// Forgets tabs for notes that are gone.
    fn prune_tabs(self: &Rc<Self>) {
        let current = self.tabs.borrow().get(self.tab.get()).cloned();
        self.tabs.borrow_mut().retain(|path| self.vault.borrow().get(path).is_some());
        let tabs = self.tabs.borrow();
        let index = current.and_then(|current| tabs.iter().position(|tab| *tab == current)).unwrap_or(0);
        self.tab.set(index.min(tabs.len().saturating_sub(1)));
        drop(tabs);
        self.refresh_tabs();
    }

    fn quick_open(self: &Rc<Self>, new_tab: bool) {
        if self.switcher.is_shown() && self.switch_to_new_tab.get() == new_tab {
            self.switcher.hide();
            return;
        }
        self.switch_to_new_tab.set(new_tab);
        self.switcher.show(&self.vault.borrow());
    }

    fn show_view(self: &Rc<Self>, graph: bool) {
        if graph {
            self.graph.show(&self.vault.borrow(), self.current.borrow().as_deref());
            self.stack.set_visible_child_name("graph");
        } else {
            self.stack.set_visible_child_name(if self.current.borrow().is_some() { "note" } else { "empty" });
        }
    }

    fn connect_find(self: &Rc<Self>, previous: &gtk::Button, next: &gtk::Button, close: &gtk::Button, replace_one: &gtk::Button, replace_all: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.find_entry.connect_search_changed(move |entry| {
            if let Some(notes) = weak.upgrade() {
                notes.editor.find(&entry.text());
                notes.found(notes.editor.match_from_cursor());
            }
        });
        let step = |notes: &Rc<Self>, forward: bool| {
            let count = notes.editor.match_count();
            if count > 0 {
                let index = notes.find_index.get();
                notes.found(if forward { (index + 1) % count } else { (index + count - 1) % count });
            }
        };
        let weak = Rc::downgrade(self);
        self.find_entry.connect_activate(move |_| {
            if let Some(notes) = weak.upgrade() {
                step(&notes, true);
            }
        });
        let weak = Rc::downgrade(self);
        self.find_entry.connect_next_match(move |_| {
            if let Some(notes) = weak.upgrade() {
                step(&notes, true);
            }
        });
        let weak = Rc::downgrade(self);
        self.find_entry.connect_previous_match(move |_| {
            if let Some(notes) = weak.upgrade() {
                step(&notes, false);
            }
        });
        let weak = Rc::downgrade(self);
        next.connect_clicked(move |_| {
            if let Some(notes) = weak.upgrade() {
                step(&notes, true);
            }
        });
        let weak = Rc::downgrade(self);
        previous.connect_clicked(move |_| {
            if let Some(notes) = weak.upgrade() {
                step(&notes, false);
            }
        });
        let weak = Rc::downgrade(self);
        self.find_entry.connect_stop_search(move |_| {
            if let Some(notes) = weak.upgrade() {
                notes.hide_find();
            }
        });
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(notes) = weak.upgrade() {
                notes.hide_find();
            }
        });
        let weak = Rc::downgrade(self);
        replace_one.connect_clicked(move |_| {
            let Some(notes) = weak.upgrade() else { return };
            notes.editor.replace_match(notes.find_index.get(), &notes.replace_entry.text());
            let count = notes.editor.match_count();
            notes.found(notes.find_index.get().min(count.saturating_sub(1)));
        });
        let weak = Rc::downgrade(self);
        replace_all.connect_clicked(move |_| {
            let Some(notes) = weak.upgrade() else { return };
            let replaced = notes.editor.replace_all(&notes.replace_entry.text());
            notes.find_count.set_label(&format!("Replaced {replaced}"));
        });
    }

    fn show_find(self: &Rc<Self>, replace: bool) {
        if self.current.borrow().is_none() {
            return;
        }
        self.note_view.set_active(true);
        self.replace_row.set_visible(replace);
        self.find.set_reveal_child(true);
        // Start from the selection, as most editors do.
        if let Some((start, end)) = self.editor.buffer.selection_bounds() {
            let selected = self.editor.buffer.text(&start, &end, false);
            if !selected.contains('\n') {
                self.find_entry.set_text(&selected);
            }
        }
        self.find_entry.grab_focus();
        self.editor.find(&self.find_entry.text());
        self.found(self.editor.match_from_cursor());
    }

    fn hide_find(self: &Rc<Self>) {
        self.find.set_reveal_child(false);
        self.editor.find("");
        self.editor.view.grab_focus();
    }

    /// Selects match `index` and shows where it is in the count.
    fn found(&self, index: usize) {
        let count = self.editor.match_count();
        self.find_index.set(index);
        if count == 0 {
            self.find_count.set_label(if self.find_entry.text().is_empty() { "" } else { "No matches" });
            return;
        }
        self.editor.select_match(index);
        self.find_count.set_label(&format!("{} of {count}", index + 1));
    }

    fn refresh_backlinks(self: &Rc<Self>) {
        self.backlinks.remove_all();
        let Some(current) = self.current.borrow().clone() else { return };
        let vault = self.vault.borrow();
        let links = vault.backlinks(&current);
        self.backlinks_title.set_label(&format!("LINKED FROM ({})", links.len()));
        if links.is_empty() {
            let none = gtk::Label::new(Some("No other note links here yet"));
            none.add_css_class("none");
            self.backlinks.append(&none);
        }
        for note in links {
            let button = gtk::Button::with_label(&note.title);
            button.add_css_class("backlink");
            let (weak, path) = (Rc::downgrade(self), note.path.clone());
            button.connect_clicked(move |_| {
                if let Some(notes) = weak.upgrade() {
                    notes.open(&path);
                }
            });
            self.backlinks.append(&button);
        }
    }

    /// Saves shortly after typing stops.
    fn edited(self: &Rc<Self>) {
        self.dirty.set(true);
        let generation = self.save_generation.get() + 1;
        self.save_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(600), move || {
            if let Some(notes) = weak.upgrade()
                && notes.save_generation.get() == generation
            {
                notes.save();
            }
        });
    }

    pub fn save(self: &Rc<Self>) {
        if !self.dirty.replace(false) {
            return;
        }
        let Some(path) = self.current.borrow().clone() else { return };
        let text = self.editor.text();
        let full = self.vault.borrow().full_path(&path);
        if let Err(err) = write(&full, &text) {
            eprintln!("notes: saving {path}: {err}");
            self.dirty.set(true);
            return;
        }
        if let Some(note) = self.vault.borrow_mut().get_mut(&path) {
            note.set_text(text);
            note.modified = SystemTime::now();
        }
        self.refresh_sidebar();
        self.refresh_list();
    }

    pub fn new_note(self: &Rc<Self>) {
        self.save();
        let folder = self.folder.borrow().clone().unwrap_or_default();
        let path = self.vault.borrow().free_path(&folder, "Untitled");
        if !self.create(&path, "") {
            return;
        }
        self.tag.replace(None);
        self.search.set_text("");
        self.refresh_sidebar();
        self.open(&path);
        self.fresh.replace(Some(path));
        self.editor.title.grab_focus();
        self.editor.title.select_region(0, -1);
    }

    /// Removes the note made with New if it's still empty when another note opens.
    fn drop_fresh(self: &Rc<Self>, opening: &str) {
        let Some(fresh) = self.fresh.take() else { return };
        let empty = self.vault.borrow().get(&fresh).is_some_and(|note| note.text.trim().is_empty());
        if fresh == opening || !empty {
            return;
        }
        let full = self.vault.borrow().full_path(&fresh);
        if std::fs::read_to_string(&full).is_ok_and(|text| text.trim().is_empty()) && std::fs::remove_file(&full).is_ok() {
            self.vault.borrow_mut().notes.retain(|note| note.path != fresh);
            self.tabs.borrow_mut().retain(|tab| *tab != fresh);
            self.refresh_sidebar();
        }
    }

    /// Writes a new note to disk and adds it to the vault.
    fn create(&self, path: &str, text: &str) -> bool {
        let full = self.vault.borrow().full_path(path);
        if let Err(err) = write(&full, text) {
            eprintln!("notes: creating {path}: {err}");
            return false;
        }
        let title = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".md").to_owned();
        let folder = path.rsplit_once('/').map(|(folder, _)| folder.to_owned()).unwrap_or_default();
        self.vault.borrow_mut().notes.push(Note::new(path.to_owned(), title, folder, SystemTime::now(), text.to_owned()));
        true
    }

    /// Opens the note a link points at, creating it in the vault's top folder if it doesn't
    /// exist yet, as Obsidian does.
    pub fn follow(self: &Rc<Self>, target: &str, new_tab: bool) {
        self.save();
        let found = self.vault.borrow().resolve(target).map(|note| note.path.clone());
        let path = match found {
            Some(path) => path,
            None => {
                let title = target.replace('/', "-");
                let path = format!("{title}.md");
                if title.starts_with('.') || !self.create(&path, "") {
                    return;
                }
                self.refresh_sidebar();
                path
            }
        };
        self.show(&path, new_tab);
    }

    /// Renames the open note's file and points every link to it at the new name.
    fn rename(self: &Rc<Self>, title: &str) {
        let Some(old_path) = self.current.borrow().clone() else { return };
        let (old_title, folder) = match self.vault.borrow().get(&old_path) {
            Some(note) => (note.title.clone(), note.folder.clone()),
            None => return,
        };
        let new_path = if folder.is_empty() { format!("{title}.md") } else { format!("{folder}/{title}.md") };
        let taken = new_path != old_path && (self.vault.borrow().get(&new_path).is_some() || self.vault.borrow().full_path(&new_path).exists());
        if title == old_title || title.is_empty() || title.contains('/') || title.starts_with('.') || taken {
            self.editor.title.set_text(&old_title);
            return;
        }
        self.save();
        let (old_full, new_full) = (self.vault.borrow().full_path(&old_path), self.vault.borrow().full_path(&new_path));
        if let Err(err) = std::fs::rename(&old_full, &new_full) {
            eprintln!("notes: renaming {old_path}: {err}");
            self.editor.title.set_text(&old_title);
            return;
        }
        let linking: Vec<String> = self.vault.borrow().backlinks(&old_path).iter().map(|note| note.path.clone()).collect();
        {
            let mut vault = self.vault.borrow_mut();
            if let Some(note) = vault.get_mut(&old_path) {
                note.path = new_path.clone();
                note.title = title.to_owned();
            }
            for path in linking {
                let full = vault.full_path(&path);
                let Some(note) = vault.get_mut(&path) else { continue };
                let Some(text) = Vault::relink(&note.text, &old_title, title) else { continue };
                match write(&full, &text) {
                    Ok(()) => note.set_text(text),
                    Err(err) => eprintln!("notes: updating links in {path}: {err}"),
                }
            }
        }
        {
            let mut state = self.state.borrow_mut();
            for pinned in state.pinned.iter_mut().filter(|pinned| **pinned == old_path) {
                *pinned = new_path.clone();
            }
            for tab in self.tabs.borrow_mut().iter_mut().filter(|tab| **tab == old_path) {
                *tab = new_path.clone();
            }
            state.open = Some(new_path.clone());
            state.tabs = self.tabs.borrow().clone();
            state.save(&self.vault.borrow().root);
        }
        self.current.replace(Some(new_path.clone()));
        self.refresh_tabs();
        self.path.set_label(&format!("{}/{new_path}", display(&self.vault.borrow().root)));
        self.refresh_sidebar();
        self.refresh_list();
        self.refresh_backlinks();
    }

    /// Follows changes made outside: another editor, git, Obsidian.
    fn watch(self: &Rc<Self>) {
        let vault = self.vault.borrow();
        let dirs = std::iter::once(vault.root.clone()).chain(vault.folders.iter().map(|folder| vault.root.join(folder)));
        let mut monitors = Vec::new();
        for dir in dirs {
            let Ok(monitor) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
                continue;
            };
            let weak = Rc::downgrade(self);
            monitor.connect_changed(move |_, file, other, event| {
                let Some(notes) = weak.upgrade() else { return };
                let file = if event == gio::FileMonitorEvent::Renamed { other.unwrap_or(file) } else { file };
                if notes.is_own_write(file, event) {
                    return;
                }
                notes.schedule_rescan();
            });
            monitors.push(monitor);
        }
        self.monitors.replace(monitors);
    }

    /// Our own saves: temporary files, and notes whose content matches what we hold.
    fn is_own_write(&self, file: &gio::File, event: gio::FileMonitorEvent) -> bool {
        let Some(path) = file.path() else { return true };
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') {
            return true;
        }
        if event == gio::FileMonitorEvent::Deleted || event == gio::FileMonitorEvent::MovedOut {
            return false;
        }
        let vault = self.vault.borrow();
        let Ok(rel) = path.strip_prefix(&vault.root) else { return true };
        let Some(note) = vault.get(&rel.to_string_lossy()) else { return !name.ends_with(".md") && !path.is_dir() };
        std::fs::read_to_string(&path).is_ok_and(|text| text == note.text)
    }

    fn schedule_rescan(self: &Rc<Self>) {
        if self.rescan_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if let Some(notes) = weak.upgrade() {
                notes.rescan_pending.set(false);
                notes.rescan();
            }
        });
    }

    fn rescan(self: &Rc<Self>) {
        let root = self.vault.borrow().root.clone();
        *self.vault.borrow_mut() = Vault::scan(&root);
        let current = self.current.borrow().clone();
        if let Some(path) = current {
            let text = self.vault.borrow().get(&path).map(|note| note.text.clone());
            match text {
                Some(text) if !self.dirty.get() && text != self.editor.text() => self.editor.reload(&text),
                Some(_) => {}
                None => {
                    self.current.replace(None);
                    self.dirty.set(false);
                    self.stack.set_visible_child_name("empty");
                }
            }
        }
        self.prune_tabs();
        self.watch();
        self.refresh_sidebar();
        self.refresh_list();
        self.refresh_backlinks();
        if self.graph_view.is_active() {
            self.show_view(true);
        }
    }
}

/// Writes through a hidden temporary file and a rename, so a crash never leaves half a note.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    autark_common::files::write_atomically(path, text.as_bytes())
}

/// A path with the home folder shown as `~`.
pub fn display(path: &Path) -> String {
    let home = crate::config::home();
    match path.strip_prefix(&home) {
        Ok(rest) => PathBuf::from("~").join(rest).to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Today's notes show the time, this year's the day, older ones the year too.
fn short_date(time: SystemTime) -> String {
    let time = chrono::DateTime::<chrono::Local>::from(time);
    let now = chrono::Local::now();
    if time.date_naive() == now.date_naive() {
        time.format("%H:%M").to_string()
    } else if time.format("%Y").to_string() == now.format("%Y").to_string() {
        time.format("%-d %b").to_string()
    } else {
        time.format("%-d %b %Y").to_string()
    }
}
