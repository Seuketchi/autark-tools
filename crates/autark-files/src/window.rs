//! One Files window: sidebar, toolbar (back, path, view switch, search), the icon or list view,
//! the info pane and the status bar.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use autark_common::files;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use crate::drives;
use crate::entry::{self, Entry};
use crate::info::Info;
use crate::ops::{self, Op};
use crate::search;
use crate::sidebar::{self, Glyph};
use crate::views::{self, Hooks};
use crate::{App, Clip};

#[derive(Clone, PartialEq, Debug)]
pub enum Location {
    Folder(PathBuf),
    Trash,
}

/// How long a message stays in the status bar.
const MESSAGE: Duration = Duration::from_secs(6);

pub struct Window {
    pub window: gtk::ApplicationWindow,
    app: Rc<App>,
    location: RefCell<Location>,
    history: RefCell<Vec<Location>>,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    views: gtk::Stack,
    grid: gtk::GridView,
    list: gtk::ColumnView,
    sidebar: gtk::Box,
    crumbs: gtk::Box,
    back: gtk::Button,
    empty_trash: gtk::Button,
    grid_button: gtk::ToggleButton,
    list_button: gtk::ToggleButton,
    search: gtk::SearchEntry,
    info: Info,
    status: gtk::Label,
    undo: gtk::Button,
    message: RefCell<Option<glib::SourceId>>,
    last_trashed: RefCell<Vec<PathBuf>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    reload_pending: RefCell<Option<glib::SourceId>>,
    searching: RefCell<Option<Arc<AtomicBool>>>,
    search_generation: Cell<u64>,
    /// Set while navigating, so clearing the search box doesn't reload twice.
    navigating: Cell<bool>,
    rename: gtk::Popover,
    rename_entry: gtk::Entry,
    me: Weak<Window>,
}

impl Window {
    pub fn new(app: &gtk::Application, shared: &Rc<App>, start: Location) -> Rc<Self> {
        let this = Rc::new_cyclic(|me: &Weak<Window>| Self::build(app, shared, me.clone()));
        this.connect();
        this.add_actions();
        this.go(start, false);
        shared.windows.borrow_mut().push(this.clone());
        this
    }

    fn build(app: &gtk::Application, shared: &Rc<App>, me: Weak<Window>) -> Self {
        let window = gtk::ApplicationWindow::builder().application(app).default_width(1200).default_height(712).title("Files").build();
        window.add_css_class("files");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let hooks = Rc::new(Hooks {
            on_menu: Rc::new({
                let me = me.clone();
                move |position, widget, x, y| {
                    if let Some(this) = me.upgrade() {
                        this.context_menu(position, widget, x, y);
                    }
                }
            }),
            on_drop: Rc::new({
                let me = me.clone();
                move |paths, folder, copy| me.upgrade().is_some_and(|this| this.dropped(paths, folder, copy))
            }),
            drag_files: Rc::new({
                let me = me.clone();
                move |position| me.upgrade().map(|this| this.drag_files(position)).unwrap_or_default()
            }),
            thumbnails: shared.thumbnails.clone(),
        });

        let list = views::list(&gtk::MultiSelection::new(None::<gio::ListModel>), &hooks);
        let sorted = gtk::SortListModel::new(Some(store.clone()), list.sorter());
        let selection = gtk::MultiSelection::new(Some(sorted));
        list.set_model(Some(&selection));
        let grid = views::grid(&selection, &hooks);

        let scroller = |child: &gtk::Widget| gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).hexpand(true).vexpand(true).child(child).build();
        let views = gtk::Stack::new();
        views.add_css_class("views");
        views.add_named(&scroller(grid.upcast_ref()), Some("grid"));
        views.add_named(&scroller(list.upcast_ref()), Some("list"));
        for page in [views.child_by_name("grid"), views.child_by_name("list")].into_iter().flatten() {
            let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let me = me.clone();
            click.connect_pressed(move |gesture, _, x, y| {
                if let (Some(this), Some(widget)) = (me.upgrade(), gesture.widget()) {
                    this.context_menu(None, &widget, x, y);
                }
            });
            page.add_controller(click);
        }
        let drop_me = me.clone();
        views.add_controller(views::drop_target(&hooks, move || drop_me.upgrade().and_then(|this| this.folder())));

        // Sidebar.
        let places = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.add_css_class("sidebar");
        side.set_size_request(216, -1);
        // Rows expand to fill it; that mustn't widen the sidebar itself.
        side.set_hexpand(false);
        side.append(&gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&places).build());

        // Toolbar.
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.add_css_class("back");
        back.set_tooltip_text(Some("Back (Alt+Left)"));
        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        crumbs.add_css_class("crumbs");
        crumbs.set_hexpand(true);
        let empty_trash = gtk::Button::with_label("Empty Trash");
        empty_trash.add_css_class("empty-trash");
        let grid_button = gtk::ToggleButton::builder().icon_name("view-grid-symbolic").tooltip_text("Icons (Ctrl+1)").build();
        let list_button = gtk::ToggleButton::builder().icon_name("view-list-symbolic").tooltip_text("List (Ctrl+2)").group(&grid_button).build();
        let switch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        switch.add_css_class("view-switch");
        switch.append(&grid_button);
        switch.append(&list_button);
        let search = gtk::SearchEntry::builder().placeholder_text("Search").width_chars(18).build();
        search.add_css_class("search");
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        toolbar.add_css_class("files-toolbar");
        for widget in [back.upcast_ref::<gtk::Widget>(), crumbs.upcast_ref(), empty_trash.upcast_ref(), switch.upcast_ref(), search.upcast_ref()] {
            toolbar.append(widget);
        }
        toolbar.append(&autark_common::chrome::controls(true));

        let info = Info::new(shared.thumbnails.clone());
        let rename_entry = gtk::Entry::new();
        rename_entry.set_width_chars(24);
        let rename = gtk::Popover::builder().child(&rename_entry).build();
        rename.set_parent(&info.name);

        let status = gtk::Label::new(None);
        status.add_css_class("status");
        let undo = gtk::Button::with_label("Undo");
        undo.add_css_class("undo");
        undo.set_visible(false);
        let status_bar = gtk::CenterBox::new();
        status_bar.add_css_class("status-bar");
        let status_middle = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_middle.append(&status);
        status_middle.append(&undo);
        status_bar.set_center_widget(Some(&status_middle));

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        body.append(&views);
        body.append(&gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).propagate_natural_width(true).child(&info.root).build());
        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.set_hexpand(true);
        main.add_css_class("main");
        main.append(&gtk::WindowHandle::builder().child(&toolbar).build());
        main.append(&body);
        main.append(&status_bar);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.append(&side);
        content.append(&main);
        window.set_child(Some(&content));

        let list_view = shared.state.borrow().list_view;
        views.set_visible_child_name(if list_view { "list" } else { "grid" });
        list_button.set_active(list_view);
        grid_button.set_active(!list_view);

        Self {
            window,
            app: shared.clone(),
            location: RefCell::new(Location::Folder(glib::home_dir())),
            history: RefCell::default(),
            store,
            selection,
            views,
            grid,
            list,
            sidebar: places,
            crumbs,
            back,
            empty_trash,
            grid_button,
            list_button,
            search,
            info,
            status,
            undo,
            message: RefCell::default(),
            last_trashed: RefCell::default(),
            monitor: RefCell::default(),
            reload_pending: RefCell::default(),
            searching: RefCell::default(),
            search_generation: Cell::new(0),
            navigating: Cell::new(false),
            rename,
            rename_entry,
            me,
        }
    }

    fn connect(&self) {
        let me = self.me.clone();
        self.selection.connect_selection_changed(move |_, _, _| {
            if let Some(this) = me.upgrade() {
                this.selection_changed();
            }
        });
        // Items vanish from the selection when the folder changes under it.
        let me = self.me.clone();
        self.selection.connect_items_changed(move |_, _, _, _| {
            if let Some(this) = me.upgrade() {
                this.selection_changed();
            }
        });
        let me = self.me.clone();
        self.grid.connect_activate(move |_, position| {
            if let Some(this) = me.upgrade() {
                this.activate(position);
            }
        });
        let me = self.me.clone();
        self.list.connect_activate(move |_, position| {
            if let Some(this) = me.upgrade() {
                this.activate(position);
            }
        });
        let me = self.me.clone();
        self.back.connect_clicked(move |_| {
            if let Some(this) = me.upgrade() {
                this.go_back();
            }
        });
        self.empty_trash.set_action_name(Some("win.empty-trash"));
        for (button, list) in [(&self.grid_button, false), (&self.list_button, true)] {
            let me = self.me.clone();
            button.connect_toggled(move |button| {
                if button.is_active()
                    && let Some(this) = me.upgrade()
                {
                    this.set_list_view(list);
                }
            });
        }
        let me = self.me.clone();
        self.search.connect_search_changed(move |_| {
            if let Some(this) = me.upgrade()
                && !this.navigating.get()
            {
                this.reload();
            }
        });
        let me = self.me.clone();
        self.search.connect_stop_search(move |search| {
            search.set_text("");
            if let Some(this) = me.upgrade() {
                this.focus_view();
            }
        });
        let me = self.me.clone();
        self.info.open.connect_clicked(move |_| {
            if let Some(this) = me.upgrade() {
                this.open_selected();
            }
        });
        let me = self.me.clone();
        self.undo.connect_clicked(move |_| {
            if let Some(this) = me.upgrade() {
                this.undo_trash();
            }
        });
        let me = self.me.clone();
        self.rename_entry.connect_activate(move |entry| {
            if let Some(this) = me.upgrade() {
                this.finish_rename(entry.text().trim());
            }
        });

        // Keys that would steal from text fields stay on the views.
        let keys = gtk::ShortcutController::new();
        for (trigger, action) in [("Delete", "win.trash"), ("F2", "win.rename"), ("<Control>c", "win.copy"), ("<Control>x", "win.cut"), ("<Control>v", "win.paste"), ("BackSpace", "win.back")] {
            keys.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string(trigger), Some(gtk::NamedAction::new(action))));
        }
        self.views.add_controller(keys);

        let me = self.me.clone();
        self.window.connect_close_request(move |_| {
            if let Some(this) = me.upgrade() {
                this.rename.unparent();
                this.app.windows.borrow_mut().retain(|window| !Rc::ptr_eq(window, &this));
            }
            glib::Propagation::Proceed
        });
    }

    fn add_actions(&self) {
        let add = |name: &str, run: fn(&Window)| {
            let action = gio::SimpleAction::new(name, None);
            let me = self.me.clone();
            action.connect_activate(move |_, _| {
                if let Some(this) = me.upgrade() {
                    run(&this);
                }
            });
            self.window.add_action(&action);
        };
        add("open", Window::open_selected);
        add("new-folder", Window::new_folder);
        add("rename", Window::begin_rename);
        add("copy", |this| this.copy(false));
        add("cut", |this| this.copy(true));
        add("paste", Window::paste);
        add("trash", Window::trash_selected);
        add("compress", |this| this.compress(false));
        add("compress-tar", |this| this.compress(true));
        add("extract-here", Window::extract_here);
        add("put-back", Window::put_back);
        add("delete", Window::delete_selected);
        add("empty-trash", Window::empty_trash);
        add("show-in-folder", Window::show_in_folder);
        add("back", Window::go_back);
        add("up", Window::go_up);
        add("new-window", |this| {
            if let Some(app) = this.window.application() {
                Window::new(&app, &this.app, this.location.borrow().clone()).window.present();
            }
        });
        add("close", |this| this.window.close());
        add("find", |this| {
            this.search.grab_focus();
        });
        add("view-grid", |this| this.grid_button.set_active(true));
        add("view-list", |this| this.list_button.set_active(true));
        add("show-hidden", |this| {
            let hidden = !this.app.state.borrow().show_hidden;
            this.app.state.borrow_mut().show_hidden = hidden;
            this.app.state.borrow().save();
            this.app.each_window(Window::reload);
        });

        let with_path = |name: &str, run: fn(&Window, &str)| {
            let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let me = self.me.clone();
            action.connect_activate(move |_, parameter| {
                if let (Some(this), Some(value)) = (me.upgrade(), parameter.and_then(|parameter| parameter.get::<String>())) {
                    run(&this, &value);
                }
            });
            self.window.add_action(&action);
        };
        with_path("open-with", Window::open_with);
        with_path("add-favourite", |this, path| {
            this.app.state.borrow_mut().favourites.push(entry::tilde(Path::new(path)));
            this.app.state.borrow().save();
            this.app.each_window(Window::refresh_sidebar);
        });
        with_path("remove-favourite", |this, path| {
            let path = entry::tilde(Path::new(path));
            this.app.state.borrow_mut().favourites.retain(|saved| entry::tilde(&crate::state::expand(saved)) != path);
            this.app.state.borrow().save();
            this.app.each_window(Window::refresh_sidebar);
        });
        with_path("eject", Window::eject);
        with_path("go", |this, path| this.go(Location::Folder(PathBuf::from(path)), true));
    }

    // Navigation.

    pub fn go(&self, location: Location, remember: bool) {
        let current = self.location.borrow().clone();
        if remember && current != location {
            self.history.borrow_mut().push(current);
        }
        self.navigating.set(true);
        self.search.set_text("");
        self.navigating.set(false);
        self.location.replace(location);
        self.watch();
        self.refresh_toolbar();
        self.refresh_sidebar();
        self.reload();
        self.focus_view();
    }

    fn go_back(&self) {
        let previous = self.history.borrow_mut().pop();
        if let Some(previous) = previous {
            self.go(previous, false);
        }
    }

    fn go_up(&self) {
        if let Some(parent) = self.folder().and_then(|folder| folder.parent().map(Path::to_path_buf)) {
            self.go(Location::Folder(parent), true);
        }
    }

    /// The folder shown, or `None` in the Trash.
    fn folder(&self) -> Option<PathBuf> {
        match &*self.location.borrow() {
            Location::Folder(path) => Some(path.clone()),
            Location::Trash => None,
        }
    }

    fn in_trash(&self) -> bool {
        *self.location.borrow() == Location::Trash
    }

    fn focus_view(&self) {
        if self.views.visible_child_name().as_deref() == Some("list") {
            self.list.grab_focus();
        } else {
            self.grid.grab_focus();
        }
    }

    fn set_list_view(&self, list: bool) {
        self.views.set_visible_child_name(if list { "list" } else { "grid" });
        self.app.state.borrow_mut().list_view = list;
        self.app.state.borrow().save();
        self.focus_view();
    }

    fn refresh_toolbar(&self) {
        self.back.set_sensitive(!self.history.borrow().is_empty());
        self.empty_trash.set_visible(self.in_trash());
        while let Some(child) = self.crumbs.first_child() {
            self.crumbs.remove(&child);
        }
        let mut parts: Vec<(String, Option<PathBuf>)> = Vec::new();
        match &*self.location.borrow() {
            Location::Trash => parts.push(("Trash".to_owned(), None)),
            Location::Folder(path) => {
                let home = glib::home_dir();
                let mut at = path.clone();
                loop {
                    let name = if at == home {
                        "Home".to_owned()
                    } else if at == Path::new("/") {
                        "Computer".to_owned()
                    } else {
                        at.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
                    };
                    parts.push((name, Some(at.clone())));
                    if at == home || parts.len() == 4 {
                        break;
                    }
                    match at.parent() {
                        Some(parent) => at = parent.to_path_buf(),
                        None => break,
                    }
                }
                parts.reverse();
            }
        }
        self.window.set_title(Some(parts.last().map_or("Files", |(name, _)| name.as_str())));
        let last = parts.len().saturating_sub(1);
        for (n, (name, path)) in parts.into_iter().enumerate() {
            if n > 0 {
                let separator = gtk::Label::new(Some("›"));
                separator.add_css_class("separator");
                self.crumbs.append(&separator);
            }
            let crumb = gtk::Button::with_label(&name);
            crumb.add_css_class("crumb");
            if n == last {
                crumb.add_css_class("current");
            }
            if let Some(path) = path.filter(|_| n != last) {
                crumb.set_action_name(Some("win.go"));
                crumb.set_action_target_value(Some(&path.to_string_lossy().to_variant()));
            }
            self.crumbs.append(&crumb);
        }
    }

    pub fn refresh_sidebar(&self) {
        while let Some(child) = self.sidebar.first_child() {
            self.sidebar.remove(&child);
        }
        let location = self.location.borrow().clone();
        self.sidebar.append(&sidebar::heading("FAVOURITES"));
        let favourites = sidebar::favourites(&self.app.state.borrow().favourites);
        for (name, path) in favourites {
            let row = sidebar::row(sidebar::glyph_for(&path), &name, None);
            if location == Location::Folder(path.clone()) {
                row.add_css_class("current");
            }
            self.place_row(&row, Location::Folder(path.clone()));
            self.row_menu(&row, &[("Remove from Favourites", "win.remove-favourite", path.to_string_lossy().as_ref())]);
            self.sidebar.append(&row);
        }
        self.sidebar.append(&sidebar::heading("LOCATIONS"));
        for drive in drives::mounted() {
            let row = sidebar::row(Glyph::Drive, &drive.name, Some(&drive.filesystem));
            if location == Location::Folder(drive.path.clone()) {
                row.add_css_class("current");
            }
            self.place_row(&row, Location::Folder(drive.path.clone()));
            if drive.removable {
                self.row_menu(&row, &[("Eject", "win.eject", drive.path.to_string_lossy().as_ref())]);
            }
            self.sidebar.append(&row);
        }
        let trash = sidebar::row(Glyph::Trash, "Trash", None);
        if location == Location::Trash {
            trash.add_css_class("current");
        }
        self.place_row(&trash, Location::Trash);
        self.sidebar.append(&trash);
    }

    /// Clicking goes there; dropping files moves them there (to the Trash, for the Trash).
    fn place_row(&self, row: &gtk::Button, location: Location) {
        let me = self.me.clone();
        let target = location.clone();
        row.connect_clicked(move |_| {
            if let Some(this) = me.upgrade() {
                this.go(target.clone(), true);
            }
        });
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let me = self.me.clone();
        drop.connect_drop(move |_, value, _, _| {
            let Some(this) = me.upgrade() else { return false };
            let paths: Vec<PathBuf> = value.get::<gdk::FileList>().map(|list| list.files().iter().filter_map(|file| file.path()).collect()).unwrap_or_default();
            match &location {
                Location::Folder(folder) => this.dropped(paths, folder.clone(), false),
                Location::Trash => {
                    this.run(Op::Trash(paths, now()), Then::Trashed);
                    true
                }
            }
        });
        row.add_controller(drop);
    }

    /// A right-click menu of `(label, action, path)` items on a sidebar row.
    fn row_menu(&self, row: &gtk::Button, items: &[(&str, &str, &str)]) {
        let menu = gio::Menu::new();
        for (label, action, path) in items {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some(action), Some(&path.to_variant()));
            menu.append_item(&item);
        }
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let me = self.me.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            if let (Some(this), Some(widget)) = (me.upgrade(), gesture.widget()) {
                this.popup(&menu, &widget, x, y);
            }
        });
        row.add_controller(click);
    }

    // Contents.

    fn watch(&self) {
        let dir = match &*self.location.borrow() {
            Location::Folder(path) => path.clone(),
            Location::Trash => files::home_trash().join("files"),
        };
        let monitor = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE).ok();
        if let Some(monitor) = &monitor {
            let me = self.me.clone();
            monitor.connect_changed(move |_, _, _, _| {
                if let Some(this) = me.upgrade() {
                    this.reload_soon();
                }
            });
        }
        if let Some(old) = self.monitor.replace(monitor) {
            old.cancel();
        }
    }

    fn reload_soon(&self) {
        if let Some(pending) = self.reload_pending.take() {
            pending.remove();
        }
        let me = self.me.clone();
        *self.reload_pending.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_millis(200), move || {
            if let Some(this) = me.upgrade() {
                this.reload_pending.take();
                // A search keeps its results until it's run again.
                if this.search.text().is_empty() {
                    this.reload_keeping_selection();
                }
            }
        }));
    }

    fn reload_keeping_selection(&self) {
        let keep: Vec<PathBuf> = self.selected().into_iter().map(|entry| entry.path).collect();
        self.reload();
        self.select_paths(&keep);
    }

    pub fn reload(&self) {
        let query = self.search.text().trim().to_owned();
        let hidden = self.app.state.borrow().show_hidden;
        let location = self.location.borrow().clone();
        match location {
            Location::Trash => {
                let mounts: Vec<PathBuf> = drives::mounted().into_iter().map(|drive| drive.path).collect();
                let lower = query.to_lowercase();
                let entries = files::trashed(&mounts).iter().filter_map(Entry::trashed).filter(|entry| entry.name.to_lowercase().contains(&lower)).collect();
                self.fill(entries);
            }
            Location::Folder(folder) if !query.is_empty() => self.search_in(&folder, &query, hidden),
            Location::Folder(folder) => match entry::list(&folder, hidden) {
                Ok(entries) => self.fill(entries),
                Err(err) => {
                    self.fill(Vec::new());
                    self.say(&format!("Can't open “{}”: {err}", entry::tilde(&folder)), false);
                }
            },
        }
    }

    fn fill(&self, entries: Vec<Entry>) {
        let objects: Vec<glib::BoxedAnyObject> = entries.into_iter().map(views::object).collect();
        self.store.splice(0, self.store.n_items(), &objects);
        self.selection_changed();
    }

    fn search_in(&self, folder: &Path, query: &str, hidden: bool) {
        if let Some(previous) = self.searching.take() {
            previous.store(true, Ordering::Relaxed);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *self.searching.borrow_mut() = Some(cancelled.clone());
        let generation = self.search_generation.get() + 1;
        self.search_generation.set(generation);
        let (me, folder, query) = (self.me.clone(), folder.to_path_buf(), query.to_owned());
        glib::spawn_future_local(async move {
            let (root, words) = (folder.clone(), query.clone());
            let Ok(hits) = gio::spawn_blocking(move || search::walk(&root, &words, hidden, true, &cancelled)).await else { return };
            let current = |me: &Weak<Window>| me.upgrade().filter(|this| this.search_generation.get() == generation);
            let show = |this: &Window, hits: &[search::Hit]| {
                this.fill(hits.iter().filter_map(|hit| Entry::read(&hit.path)).collect());
            };
            // The code's matches first; Jev's order replaces them if it answers.
            let Some(this) = current(&me) else { return };
            show(&this, &search::named(hits.clone()));
            drop(this);
            if let Some(ranked) = search::rank(&query, &folder, &hits).await
                && let Some(this) = current(&me)
            {
                show(&this, &ranked);
            }
        });
    }

    fn selected(&self) -> Vec<Entry> {
        let bits = self.selection.selection();
        (0..bits.size()).filter_map(|n| self.selection.item(bits.nth(n as u32)).as_ref().and_then(views::entry_of)).collect()
    }

    fn entry_at(&self, position: u32) -> Option<Entry> {
        self.selection.item(position).as_ref().and_then(views::entry_of)
    }

    fn select_paths(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let mut first = true;
        for position in 0..self.selection.n_items() {
            if self.entry_at(position).is_some_and(|entry| paths.contains(&entry.path)) {
                self.selection.select_item(position, first);
                if first {
                    self.grid.scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
                }
                first = false;
            }
        }
    }

    fn selection_changed(&self) {
        self.info.show(&self.selected());
        if self.message.borrow().is_none() {
            self.show_counts();
        }
    }

    fn show_counts(&self) {
        let total = self.selection.n_items();
        let selected = self.selection.selection().size();
        let text = if !self.search.text().is_empty() {
            match total {
                1 => "1 result".to_owned(),
                n => format!("{n} results"),
            }
        } else if self.in_trash() {
            match total {
                0 => "The Trash is empty".to_owned(),
                1 => "1 item in the Trash".to_owned(),
                n => format!("{n} items in the Trash"),
            }
        } else if selected > 0 {
            format!("{selected} of {total} selected")
        } else {
            let items = if total == 1 { "1 item".to_owned() } else { format!("{total} items") };
            match self.folder().and_then(|folder| drives::free_space(&folder)) {
                Some(free) => format!("{items}, {} available", entry::size(free)),
                None => items,
            }
        };
        self.status.set_label(&text);
        self.undo.set_visible(false);
    }

    /// A message in the status bar for a few seconds, with Undo when `undo` is set.
    fn say(&self, text: &str, undo: bool) {
        if let Some(previous) = self.message.take() {
            previous.remove();
        }
        self.status.set_label(text);
        self.undo.set_visible(undo);
        let me = self.me.clone();
        *self.message.borrow_mut() = Some(glib::timeout_add_local_once(MESSAGE, move || {
            if let Some(this) = me.upgrade() {
                this.message.take();
                this.show_counts();
            }
        }));
    }

    // Opening.

    fn activate(&self, position: u32) {
        if let Some(entry) = self.entry_at(position) {
            self.open(&entry);
        }
    }

    fn open_selected(&self) {
        let selected = self.selected();
        if selected.iter().any(|entry| entry.trashed.is_some()) {
            self.put_back();
            return;
        }
        match selected.as_slice() {
            [one] => self.open(one),
            many => many.iter().filter(|entry| !entry.is_dir).for_each(|entry| self.open(entry)),
        }
    }

    fn open(&self, entry: &Entry) {
        if entry.trashed.is_some() {
            return;
        }
        if entry.is_dir {
            self.go(Location::Folder(entry.path.clone()), true);
            return;
        }
        let context = WidgetExt::display(&self.window).app_launch_context();
        let uri = gio::File::for_path(&entry.path).uri();
        if gio::AppInfo::launch_default_for_uri(&uri, Some(&context)).is_err() {
            self.say(&format!("No app opens “{}”; right-click it for Open With", entry.name), false);
        }
    }

    fn open_with(&self, app_id: &str) {
        let Some(app) = gio::AppInfo::all().into_iter().find(|app| app.id().as_deref() == Some(app_id)) else { return };
        let files: Vec<gio::File> = self.selected().iter().map(|entry| gio::File::for_path(&entry.path)).collect();
        if let Err(err) = app.launch(&files, Some(&WidgetExt::display(&self.window).app_launch_context())) {
            self.say(&format!("Couldn't open with {}: {err}", app.name()), false);
        }
    }

    fn show_in_folder(&self) {
        let Some(entry) = self.selected().into_iter().next() else { return };
        let Some(parent) = entry.path.parent() else { return };
        self.go(Location::Folder(parent.to_path_buf()), true);
        self.select_paths(&[entry.path]);
    }

    // Menus.

    fn context_menu(&self, position: Option<u32>, widget: &gtk::Widget, x: f64, y: f64) {
        match position {
            Some(position) if !self.selection.is_selected(position) => {
                self.selection.select_item(position, true);
            }
            None => {
                self.selection.unselect_all();
            }
            _ => {}
        }
        let selected = self.selected();
        let menu = gio::Menu::new();
        let section = |items: &[(&str, &str)]| {
            let section = gio::Menu::new();
            for (label, action) in items {
                section.append(Some(label), Some(action));
            }
            section
        };
        if self.in_trash() {
            if selected.is_empty() {
                menu.append_section(None, &section(&[("Empty Trash", "win.empty-trash")]));
            } else {
                menu.append_section(None, &section(&[("Put Back", "win.put-back"), ("Delete Immediately…", "win.delete")]));
            }
        } else if selected.is_empty() {
            let mut first = vec![("New Folder", "win.new-folder")];
            if self.app.clipboard.borrow().is_some() {
                first.push(("Paste", "win.paste"));
            }
            menu.append_section(None, &section(&first));
            let hidden = if self.app.state.borrow().show_hidden { "Hide Hidden Files" } else { "Show Hidden Files" };
            menu.append_section(None, &section(&[(hidden, "win.show-hidden"), ("Icons", "win.view-grid"), ("List", "win.view-list")]));
        } else {
            let open = section(&[("Open", "win.open")]);
            if let Some(with) = self.open_with_menu(&selected) {
                open.append_submenu(Some("Open With"), &with);
            }
            menu.append_section(None, &open);
            let mut edit = vec![("Copy", "win.copy"), ("Cut", "win.cut")];
            if selected.len() == 1 {
                edit.push(("Rename…", "win.rename"));
            }
            menu.append_section(None, &section(&edit));
            let mut pack = vec![("Compress", "win.compress"), ("Compress as tar.gz", "win.compress-tar")];
            if selected.iter().all(|entry| !entry.is_dir && ops::is_archive(&entry.path)) {
                pack.insert(0, ("Extract Here", "win.extract-here"));
            }
            menu.append_section(None, &section(&pack));
            let more = gio::Menu::new();
            if let [one] = selected.as_slice() {
                if one.is_dir {
                    let path = one.path.to_string_lossy().into_owned();
                    let favourite = self.app.state.borrow().favourites.iter().any(|saved| crate::state::expand(saved) == one.path);
                    let (label, action) = if favourite { ("Remove from Favourites", "win.remove-favourite") } else { ("Add to Favourites", "win.add-favourite") };
                    let item = gio::MenuItem::new(Some(label), None);
                    item.set_action_and_target_value(Some(action), Some(&path.to_variant()));
                    more.append_item(&item);
                }
                if !self.search.text().is_empty() {
                    more.append(Some("Show in Folder"), Some("win.show-in-folder"));
                }
            }
            if more.n_items() > 0 {
                menu.append_section(None, &more);
            }
            menu.append_section(None, &section(&[("Move to Trash", "win.trash")]));
        }
        self.popup(&menu, widget, x, y);
    }

    /// Apps that open every selected file, the default first.
    fn open_with_menu(&self, selected: &[Entry]) -> Option<gio::Menu> {
        if selected.iter().any(|entry| entry.is_dir) {
            return None;
        }
        let content_type = &selected.first()?.content_type;
        let apps = gio::AppInfo::all_for_type(content_type);
        let menu = gio::Menu::new();
        for app in apps.iter().filter(|app| selected.iter().all(|entry| entry.content_type == *content_type) || app.supports_files()) {
            let Some(id) = app.id() else { continue };
            let item = gio::MenuItem::new(Some(&app.display_name()), None);
            item.set_action_and_target_value(Some("win.open-with"), Some(&id.to_variant()));
            menu.append_item(&item);
        }
        (menu.n_items() > 0).then_some(menu)
    }

    fn popup(&self, menu: &gio::Menu, widget: &gtk::Widget, x: f64, y: f64) {
        let content = self.window.child().expect("window content");
        let point = widget.compute_point(&content, &graphene::Point::new(x as f32, y as f32)).unwrap_or_else(|| graphene::Point::new(x as f32, y as f32));
        let popover = gtk::PopoverMenu::from_model(Some(menu));
        popover.add_css_class("menu");
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        popover.set_parent(&content);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(point.x() as i32, point.y() as i32, 1, 1)));
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            // After the chosen action has run, which needs the popover still in place.
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }

    // Changing files.

    fn copy(&self, cut: bool) {
        let paths: Vec<PathBuf> = self.selected().into_iter().filter(|entry| entry.trashed.is_none()).map(|entry| entry.path).collect();
        if paths.is_empty() {
            return;
        }
        // The paths as text too, for pasting into a terminal or an editor.
        let text: Vec<String> = paths.iter().map(|path| path.display().to_string()).collect();
        WidgetExt::display(&self.window).clipboard().set_text(&text.join("\n"));
        let count = if paths.len() == 1 { "1 item".to_owned() } else { format!("{} items", paths.len()) };
        self.say(&format!("{count} ready to paste{}", if cut { ", moving" } else { "" }), false);
        *self.app.clipboard.borrow_mut() = Some(Clip { paths, cut });
    }

    fn paste(&self) {
        let Some(folder) = self.folder() else { return };
        let Some(clip) = self.app.clipboard.borrow().clone() else { return };
        if clip.cut {
            self.app.clipboard.borrow_mut().take();
            self.run(Op::Move(clip.paths, folder), Then::Select);
        } else {
            self.run(Op::Copy(clip.paths, folder), Then::Select);
        }
    }

    fn dropped(&self, paths: Vec<PathBuf>, folder: PathBuf, copy: bool) -> bool {
        let paths: Vec<PathBuf> = paths.into_iter().filter(|path| copy || path.parent() != Some(folder.as_path())).filter(|path| *path != folder).collect();
        if paths.is_empty() {
            return false;
        }
        self.run(if copy { Op::Copy(paths, folder) } else { Op::Move(paths, folder) }, Then::Nothing);
        true
    }

    fn drag_files(&self, position: u32) -> Vec<gio::File> {
        let entries = if self.selection.is_selected(position) { self.selected() } else { self.entry_at(position).into_iter().collect() };
        entries.iter().filter(|entry| entry.trashed.is_none()).map(|entry| gio::File::for_path(&entry.path)).collect()
    }

    fn trash_selected(&self) {
        if self.in_trash() {
            self.delete_selected();
            return;
        }
        let paths: Vec<PathBuf> = self.selected().into_iter().map(|entry| entry.path).collect();
        if !paths.is_empty() {
            self.run(Op::Trash(paths, now()), Then::Trashed);
        }
    }

    /// Packs the selection into an archive beside it.
    fn compress(&self, tar: bool) {
        let paths: Vec<PathBuf> = self.selected().into_iter().map(|entry| entry.path).collect();
        if !paths.is_empty() {
            self.run(Op::Compress(paths, tar), Then::Select);
        }
    }

    /// Unpacks each selected archive beside itself.
    fn extract_here(&self) {
        let paths: Vec<PathBuf> = self.selected().into_iter().map(|entry| entry.path).filter(|path| ops::is_archive(path)).collect();
        if !paths.is_empty() {
            self.run(Op::Extract(paths), Then::Select);
        }
    }

    fn undo_trash(&self) {
        let trashed = self.last_trashed.take();
        let mounts: Vec<PathBuf> = drives::mounted().into_iter().map(|drive| drive.path).collect();
        let items: Vec<files::Trashed> = files::trashed(&mounts).into_iter().filter(|item| trashed.contains(&item.path)).collect();
        if !items.is_empty() {
            self.run(Op::Restore(items), Then::Select);
        }
    }

    fn put_back(&self) {
        let items: Vec<files::Trashed> = self.selected().into_iter().filter_map(|entry| entry.trashed).collect();
        if !items.is_empty() {
            self.run(Op::Restore(items), Then::Nothing);
        }
    }

    fn delete_selected(&self) {
        let items: Vec<files::Trashed> = self.selected().into_iter().filter_map(|entry| entry.trashed).collect();
        if items.is_empty() {
            return;
        }
        let what = if items.len() == 1 { format!("“{}”", items[0].original.file_name().unwrap_or_default().to_string_lossy()) } else { format!("{} items", items.len()) };
        self.confirm(&format!("Delete {what} for good?"), move |this| this.run(Op::Delete(items), Then::Nothing));
    }

    fn empty_trash(&self) {
        let mounts: Vec<PathBuf> = drives::mounted().into_iter().map(|drive| drive.path).collect();
        let items = files::trashed(&mounts);
        if items.is_empty() {
            return;
        }
        let what = if items.len() == 1 { "the item".to_owned() } else { format!("all {} items", items.len()) };
        self.confirm(&format!("Delete {what} in the Trash for good?"), move |this| this.run(Op::Delete(items), Then::Nothing));
    }

    fn confirm(&self, question: &str, then: impl FnOnce(&Window) + 'static) {
        let dialog = gtk::AlertDialog::builder().message(question).detail("This can't be undone.").buttons(["Cancel", "Delete"]).cancel_button(0).default_button(0).modal(true).build();
        let me = self.me.clone();
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |answer| {
            if answer == Ok(1)
                && let Some(this) = me.upgrade()
            {
                then(&this);
            }
        });
    }

    fn new_folder(&self) {
        let Some(folder) = self.folder() else { return };
        let path = files::free_name(&folder, "untitled folder");
        if let Err(err) = std::fs::create_dir(&path) {
            self.say(&format!("Couldn't make a folder here: {err}"), false);
            return;
        }
        self.reload();
        self.select_paths(std::slice::from_ref(&path));
        self.begin_rename();
    }

    fn begin_rename(&self) {
        let Ok([entry]) = <[Entry; 1]>::try_from(self.selected()) else { return };
        if entry.trashed.is_some() {
            return;
        }
        self.rename_entry.set_text(&entry.name);
        self.rename.popup();
        self.rename_entry.grab_focus();
        // The name without its extension, ready to type over.
        let stem = if entry.is_dir { entry.name.chars().count() } else { entry.name.rfind('.').filter(|&dot| dot > 0).map_or(entry.name.chars().count(), |dot| entry.name[..dot].chars().count()) };
        self.rename_entry.select_region(0, stem as i32);
    }

    fn finish_rename(&self, name: &str) {
        self.rename.popdown();
        let Ok([entry]) = <[Entry; 1]>::try_from(self.selected()) else { return };
        if name.is_empty() || name == entry.name {
            return;
        }
        if name.contains('/') || name == "." || name == ".." {
            self.say("A name can't contain “/”", false);
            return;
        }
        let target = entry.path.with_file_name(name);
        if target.symlink_metadata().is_ok() {
            self.say(&format!("“{name}” is already taken"), false);
            return;
        }
        if let Err(err) = std::fs::rename(&entry.path, &target) {
            self.say(&format!("Couldn't rename “{}”: {err}", entry.name), false);
            return;
        }
        self.reload();
        self.select_paths(&[target]);
    }

    fn eject(&self, path: &str) {
        let (me, path) = (self.me.clone(), path.to_owned());
        glib::spawn_future_local(async move {
            let target = path.clone();
            let result = gio::spawn_blocking(move || std::process::Command::new("umount").arg(&target).output()).await;
            let Some(this) = me.upgrade() else { return };
            match result {
                Ok(Ok(output)) if output.status.success() => {
                    if this.folder().is_some_and(|folder| folder.starts_with(&path)) {
                        this.go(Location::Folder(glib::home_dir()), false);
                    }
                    this.app.each_window(Window::refresh_sidebar);
                    this.say("Ejected; it's safe to unplug", false);
                }
                Ok(Ok(output)) => this.say(&format!("Couldn't eject: {}", String::from_utf8_lossy(&output.stderr).trim()), false),
                _ => this.say("Couldn't eject: umount didn't run", false),
            }
        });
    }

    fn run(&self, op: Op, then: Then) {
        self.say("Working…", false);
        let me = self.me.clone();
        glib::spawn_future_local(async move {
            let Ok(done) = gio::spawn_blocking(move || ops::run(op)).await else { return };
            let Some(this) = me.upgrade() else { return };
            this.app.each_window(Window::reload_keeping_selection);
            match then {
                Then::Select => this.select_paths(&done.results),
                Then::Trashed => *this.last_trashed.borrow_mut() = done.results.clone(),
                Then::Nothing => {}
            }
            match done.errors.as_slice() {
                [] if done.message.is_empty() => this.show_counts(),
                [] => this.say(&done.message, matches!(then, Then::Trashed)),
                [one] => this.say(one, false),
                [first, rest @ ..] => this.say(&format!("{first} (and {} more)", rest.len()), false),
            }
        });
    }
}

/// What to do once an operation finishes.
enum Then {
    Nothing,
    /// Select what it made.
    Select,
    /// Remember what went to the Trash, for Undo.
    Trashed,
}

/// Local time as the Trash spec wants it.
fn now() -> String {
    glib::DateTime::now_local().ok().and_then(|now| now.format("%Y-%m-%dT%H:%M:%S").ok()).map(String::from).unwrap_or_default()
}
