//! The Hub's window: navigation on the left, the packages in the middle, and the chosen package on the right with what
//! can be done to it. Everything it shows comes from `autark-pkg catalog`; every change goes through the root helper.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::catalog::{self, Package, State, View};
use crate::runner;

/// What a button does when pressed, given the Hub.
type Action = Box<dyn Fn(&Rc<Hub>)>;

pub struct Hub {
    pub window: gtk::ApplicationWindow,
    packages: RefCell<Vec<Package>>,
    view: Cell<View>,
    query: RefCell<String>,
    unreviewed: Cell<bool>,
    selected: RefCell<Option<String>>,
    /// A package to choose once the catalog has loaded (`autark-hub <package>`).
    want: RefCell<Option<String>>,
    busy: Cell<bool>,
    /// Set while the list is being rebuilt, so choosing a row there is not taken as the person choosing it.
    rebuilding: Cell<bool>,

    search: gtk::SearchEntry,
    show_unreviewed: gtk::Switch,
    nav: Vec<(View, gtk::Button)>,
    updates_badge: gtk::Label,
    heading: gtk::Label,
    list: gtk::ListBox,
    pages: gtk::Stack,
    notice: gtk::Label,
    update_all: gtk::Button,
    detail: gtk::Stack,
    title: gtk::Label,
    meta: gtk::Label,
    source: gtk::Label,
    description: gtk::Label,
    depends: gtk::Label,
    kept: gtk::Label,
    actions: gtk::Box,
    status: gtk::Label,
    log: gtk::TextView,
    log_box: gtk::ScrolledWindow,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).wrap(true).selectable(false).build();
    label.add_css_class(class);
    label
}

impl Hub {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Software").default_width(1120).default_height(720).build();
        window.add_css_class("hub");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        // Navigation.
        let search = gtk::SearchEntry::builder().placeholder_text("Search").build();
        search.add_css_class("search");
        let side = gtk::Box::new(gtk::Orientation::Vertical, 2);
        side.add_css_class("sidebar");
        side.set_size_request(220, -1);
        side.append(&search);
        let updates_badge = gtk::Label::new(None);
        updates_badge.add_css_class("badge");
        updates_badge.set_visible(false);
        let mut nav = Vec::new();
        for (view, name) in [(View::Browse, "Browse"), (View::Installed, "Installed"), (View::Updates, "Updates")] {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&gtk::Label::builder().label(name).xalign(0.0).hexpand(true).build());
            if view == View::Updates {
                row.append(&updates_badge);
            }
            let button = gtk::Button::builder().child(&row).build();
            button.add_css_class("nav");
            side.append(&button);
            nav.push((view, button));
        }
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        side.append(&spacer);
        let unreviewed = gtk::Switch::builder().valign(gtk::Align::Center).build();
        unreviewed.update_property(&[gtk::accessible::Property::Label("Show unreviewed Arch packages")]);
        let option = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        option.add_css_class("option");
        let caption = label("Show unreviewed Arch packages", "note");
        caption.set_hexpand(true);
        option.append(&caption);
        option.append(&unreviewed);
        side.append(&option);

        // The list.
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        list.add_css_class("packages");
        let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let loading = label("Reading packages…", "note");
        let notice = label("", "note");
        notice.set_margin_start(24);
        notice.set_margin_end(24);
        let retry = gtk::Button::with_label("Try again");
        retry.add_css_class("plain");
        retry.set_halign(gtk::Align::Start);
        let problem = gtk::Box::new(gtk::Orientation::Vertical, 10);
        problem.set_margin_top(24);
        problem.append(&notice);
        problem.append(&retry);
        retry.set_margin_start(24);
        let empty = label("Nothing here.", "note");
        empty.set_margin_top(24);
        empty.set_margin_start(24);
        loading.set_margin_top(24);
        loading.set_margin_start(24);
        let pages = gtk::Stack::builder().vexpand(true).build();
        pages.add_named(&scroll, Some("list"));
        pages.add_named(&loading, Some("loading"));
        pages.add_named(&problem, Some("problem"));
        pages.add_named(&empty, Some("empty"));
        let update_all = gtk::Button::with_label("Update all");
        update_all.add_css_class("primary");
        update_all.set_visible(false);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("list-head");
        let heading = label("Browse", "pane-title");
        heading.set_hexpand(true);
        head.append(&heading);
        head.append(&update_all);
        let middle = gtk::Box::new(gtk::Orientation::Vertical, 0);
        middle.add_css_class("middle");
        middle.set_size_request(380, -1);
        middle.append(&head);
        middle.append(&pages);

        // The chosen package.
        let title = label("", "detail-title");
        let meta = label("", "note");
        let source = label("", "source");
        source.set_halign(gtk::Align::Start);
        let description = label("", "detail-text");
        let depends = label("", "note");
        let kept = label("", "note");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let status = label("", "status");
        status.set_visible(false);
        let log = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).build();
        log.add_css_class("log");
        let log_box = gtk::ScrolledWindow::builder().child(&log).min_content_height(180).vexpand(true).build();
        log_box.add_css_class("log-box");
        log_box.set_visible(false);
        let info = gtk::Box::new(gtk::Orientation::Vertical, 10);
        info.add_css_class("detail");
        for widget in [title.upcast_ref::<gtk::Widget>(), meta.upcast_ref(), source.upcast_ref(), description.upcast_ref(), depends.upcast_ref(), kept.upcast_ref(), actions.upcast_ref(), status.upcast_ref(), log_box.upcast_ref()] {
            info.append(widget);
        }
        let none = label("Choose a package to see what it is and what you can do with it.", "note");
        none.set_margin_top(80);
        none.set_margin_start(40);
        none.set_margin_end(40);
        let detail = gtk::Stack::builder().hexpand(true).build();
        detail.add_named(&none, Some("none"));
        detail.add_named(&info, Some("package"));
        let controls = autark_common::chrome::controls(true);
        controls.set_valign(gtk::Align::Start);
        controls.set_margin_top(10);
        controls.set_margin_end(12);
        let over = gtk::Overlay::builder().child(&detail).hexpand(true).build();
        over.add_overlay(&controls);

        let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        main.append(&side);
        main.append(&middle);
        main.append(&gtk::WindowHandle::builder().child(&over).hexpand(true).build());
        window.set_child(Some(&main));

        let hub = Rc::new(Self {
            window,
            packages: RefCell::default(),
            view: Cell::new(View::Browse),
            query: RefCell::default(),
            unreviewed: Cell::new(false),
            selected: RefCell::default(),
            want: RefCell::default(),
            busy: Cell::new(false),
            rebuilding: Cell::new(false),
            search,
            show_unreviewed: unreviewed.clone(),
            nav,
            updates_badge,
            heading,
            list,
            pages,
            notice,
            update_all,
            detail,
            title,
            meta,
            source,
            description,
            depends,
            kept,
            actions,
            status,
            log,
            log_box,
        });

        for (view, button) in &hub.nav {
            let (hub, view) = (Rc::downgrade(&hub), *view);
            button.connect_clicked(move |_| {
                if let Some(hub) = hub.upgrade() {
                    hub.show_view(view);
                }
            });
        }
        hub.search.connect_search_changed({
            let hub = Rc::downgrade(&hub);
            move |search| {
                if let Some(hub) = hub.upgrade() {
                    *hub.query.borrow_mut() = search.text().to_string();
                    hub.rebuild();
                }
            }
        });
        unreviewed.connect_active_notify({
            let hub = Rc::downgrade(&hub);
            move |switch| {
                if let Some(hub) = hub.upgrade() {
                    hub.unreviewed.set(switch.is_active());
                    hub.rebuild();
                }
            }
        });
        hub.list.connect_row_selected({
            let hub = Rc::downgrade(&hub);
            move |_, row| {
                let Some(hub) = hub.upgrade() else { return };
                if hub.rebuilding.get() {
                    return;
                }
                *hub.selected.borrow_mut() = row.map(|row| row.widget_name().to_string());
                hub.show_detail();
            }
        });
        retry.connect_clicked({
            let hub = Rc::downgrade(&hub);
            move |_| {
                if let Some(hub) = hub.upgrade() {
                    hub.reload();
                }
            }
        });
        hub.update_all.connect_clicked({
            let hub = Rc::downgrade(&hub);
            move |_| {
                if let Some(hub) = hub.upgrade() {
                    hub.job("upgrade", &[], "update every package", "Updated everything that was due.");
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let hub = Rc::downgrade(&hub);
            move |_, key, _, modifiers| {
                let Some(hub) = hub.upgrade() else { return glib::Propagation::Proceed };
                if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::f {
                    hub.search.grab_focus();
                    return glib::Propagation::Stop;
                }
                if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::w {
                    hub.window.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        hub.window.add_controller(keys);

        hub.mark_nav();
        hub.reload();
        hub
    }

    fn show_view(self: &Rc<Self>, view: View) {
        self.set_view(view);
        self.rebuild();
    }

    fn set_view(&self, view: View) {
        self.view.set(view);
        self.heading.set_label(match view {
            View::Browse => "Browse",
            View::Installed => "Installed",
            View::Updates => "Updates",
        });
        self.mark_nav();
    }

    fn mark_nav(&self) {
        for (view, button) in &self.nav {
            if *view == self.view.get() {
                button.add_css_class("current");
            } else {
                button.remove_css_class("current");
            }
        }
    }

    /// Reads the catalog again, off the main thread, and keeps the same package chosen.
    pub fn reload(self: &Rc<Self>) {
        self.pages.set_visible_child_name("loading");
        let hub = self.clone();
        glib::spawn_future_local(async move {
            let loaded = gtk::gio::spawn_blocking(catalog::load).await.unwrap_or_else(|_| Err("reading the packages stopped unexpectedly".to_owned()));
            match loaded {
                Ok(all) => {
                    let due = all.iter().filter(|package| package.upgrade.is_some()).count();
                    hub.updates_badge.set_label(&due.to_string());
                    hub.updates_badge.set_visible(due > 0);
                    *hub.packages.borrow_mut() = all;
                    hub.apply_want();
                    hub.rebuild();
                }
                Err(error) => {
                    hub.notice.set_label(&error);
                    hub.pages.set_visible_child_name("problem");
                }
            }
        });
    }

    /// Chooses the package named on the command line: in Installed if it is, else in Browse (with the Arch switch on if it
    /// is an unreviewed import), with no search in the way.
    fn apply_want(&self) {
        let Some(name) = self.want.borrow_mut().take() else { return };
        let found = self.packages.borrow().iter().find(|package| package.name == name).cloned();
        let Some(package) = found else {
            self.status.set_label(&format!("There is no package called {name}."));
            self.status.set_visible(true);
            return;
        };
        self.query.borrow_mut().clear();
        self.search.set_text("");
        if package.imported && package.installed.is_none() {
            self.unreviewed.set(true);
            self.show_unreviewed.set_active(true);
        }
        self.set_view(if package.installed.is_some() { View::Installed } else { View::Browse });
        *self.selected.borrow_mut() = Some(name);
    }

    /// Opens with `name` chosen once the packages are read (or straight away, if they already are).
    pub fn choose(self: &Rc<Self>, name: String) {
        *self.want.borrow_mut() = Some(name);
        self.reload();
    }

    /// The middle column for the current view, search and switch.
    fn rebuild(self: &Rc<Self>) {
        self.rebuilding.set(true);
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let all = self.packages.borrow();
        let shown = catalog::visible(&all, self.view.get(), &self.query.borrow(), self.unreviewed.get());
        let chosen = self.selected.borrow().clone();
        let mut again = None;
        for package in &shown {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&package.name);
            row.add_css_class("package");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_hexpand(true);
            let name = gtk::Label::builder().label(&package.name).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            name.add_css_class("package-name");
            let second = if package.description.is_empty() { format!("Version {}", package.version) } else { package.description.clone() };
            let line = gtk::Label::builder().label(second).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            line.add_css_class("note");
            text.append(&name);
            text.append(&line);
            let (word, class) = match package.state() {
                State::NotInstalled => ("", "state"),
                State::Installed => ("Installed", "state"),
                State::Update(_) => ("Update", "state-update"),
                State::Held => ("Held", "state"),
            };
            let state = gtk::Label::new(Some(word));
            state.add_css_class(class);
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            content.append(&text);
            content.append(&state);
            row.set_child(Some(&content));
            self.list.append(&row);
            if chosen.as_deref() == Some(package.name.as_str()) {
                again = Some(row);
            }
        }
        let count = shown.len();
        let updates = self.view.get() == View::Updates && count > 0;
        drop(all);
        self.update_all.set_visible(updates);
        self.update_all.set_label(&format!("Update all ({count})"));
        self.update_all.set_sensitive(!self.busy.get());
        match &again {
            Some(row) => self.list.select_row(Some(row)),
            None => self.list.unselect_all(),
        }
        self.rebuilding.set(false);
        if again.is_none() {
            *self.selected.borrow_mut() = None;
        }
        if count == 0 {
            let empty = self.pages.child_by_name("empty").and_downcast::<gtk::Label>();
            if let Some(empty) = empty {
                empty.set_label(&match (self.view.get(), self.query.borrow().is_empty()) {
                    (_, false) => "No package matches that.".to_owned(),
                    (View::Updates, _) => "Everything is up to date.".to_owned(),
                    (View::Installed, _) => "Nothing is installed.".to_owned(),
                    (View::Browse, _) => "No packages found. Is the recipes folder there?".to_owned(),
                });
            }
            self.pages.set_visible_child_name("empty");
        } else {
            self.pages.set_visible_child_name("list");
        }
        self.show_detail();
    }

    fn chosen(&self) -> Option<Package> {
        let name = self.selected.borrow().clone()?;
        self.packages.borrow().iter().find(|package| package.name == name).cloned()
    }

    fn show_detail(self: &Rc<Self>) {
        let Some(package) = self.chosen() else {
            self.detail.set_visible_child_name("none");
            return;
        };
        self.detail.set_visible_child_name("package");
        self.title.set_label(&package.name);
        let state = package.state();
        self.meta.set_label(&match &state {
            State::NotInstalled => format!("Version {}", package.version),
            State::Installed => format!("Version {} · installed", package.version),
            State::Update(next) => format!("Installed {} · {next} is available", package.installed.as_deref().unwrap_or("")),
            State::Held => format!("Installed {} · held there after a rollback; the recipe has {}", package.installed.as_deref().unwrap_or(""), package.version),
        });
        if package.imported {
            self.source.set_label("Unreviewed · imported from Arch Linux; nobody on Autark has read its recipe");
            self.source.add_css_class("warn");
        } else {
            self.source.set_label("Reviewed recipe · built on this laptop from source");
            self.source.remove_css_class("warn");
        }
        self.description.set_label(if package.description.is_empty() { "No description yet." } else { &package.description });
        self.depends.set_label(&if package.depends.is_empty() { "Depends on nothing else.".to_owned() } else { format!("Depends on {}.", package.depends.join(", ")) });
        self.kept.set_visible(!package.kept.is_empty());
        self.kept.set_label(&format!("Builds kept for rolling back: {}.", package.kept.join(", ")));
        self.fill_actions(&package, &state);
    }

    fn fill_actions(self: &Rc<Self>, package: &Package, state: &State) {
        while let Some(child) = self.actions.first_child() {
            self.actions.remove(&child);
        }
        let add = |text: String, primary: bool, run: Action| {
            let button = gtk::Button::with_label(&text);
            button.add_css_class(if primary { "primary" } else { "plain" });
            button.set_sensitive(!self.busy.get());
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(hub) = weak.upgrade() {
                    run(&hub);
                }
            });
            self.actions.append(&button);
        };
        let name = package.name.clone();
        let install = {
            let (name, imported) = (name.clone(), package.imported);
            move |hub: &Rc<Hub>| {
                let name = name.clone();
                if imported {
                    hub.confirm(
                        &format!("Install {name}, an unreviewed package?"),
                        "It was made from an Arch Linux package, not written or read by anyone on Autark. It builds here, as a user that cannot change the system, and is signed by this laptop's key; you are trusting the Arch recipe.",
                        "Install",
                        Box::new(move |hub| hub.job("install", &[name.as_str()], &format!("install {name}"), &format!("Installed {name}."))),
                    );
                } else {
                    hub.job("install", &[name.as_str()], &format!("install {name}"), &format!("Installed {name}."));
                }
            }
        };
        let remove = {
            let name = name.clone();
            move |hub: &Rc<Hub>| {
                let name = name.clone();
                hub.confirm(
                    &format!("Remove {name}?"),
                    "Packages that need it stop this: autark-pkg refuses to remove something another package depends on. You can install it again later; that builds it from its recipe.",
                    "Remove",
                    Box::new(move |hub| hub.job("remove", &[name.as_str()], &format!("remove {name}"), &format!("Removed {name}."))),
                );
            }
        };
        match state {
            State::NotInstalled => add("Install".to_owned(), true, Box::new(install)),
            State::Installed => add("Remove".to_owned(), false, Box::new(remove)),
            State::Update(next) => {
                let (n, next_version) = (name.clone(), next.clone());
                add(format!("Update to {next}"), true, Box::new(move |hub| hub.job("upgrade", &[n.as_str()], &format!("update {n}"), &format!("Updated {n} to {next_version}."))));
                add("Remove".to_owned(), false, Box::new(remove));
            }
            State::Held => {
                add(format!("Install {}", package.version), true, Box::new(install));
                add("Remove".to_owned(), false, Box::new(remove));
            }
        }
        if let Some(back) = package.rollback_to() {
            let (n, back) = (name, back.to_owned());
            add(format!("Roll back to {back}"), false, Box::new(move |hub| hub.job("rollback", &[n.as_str()], &format!("roll {n} back"), &format!("Rolled {n} back to {back}."))));
        }
    }

    fn confirm(self: &Rc<Self>, message: &str, detail: &str, go: &str, then: Action) {
        let dialog = gtk::AlertDialog::builder().modal(true).message(message).detail(detail).buttons(["Cancel", go]).cancel_button(0).default_button(0).build();
        let (hub, window) = (self.clone(), self.window.clone());
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&window)).await == Ok(1) {
                then(&hub);
            }
        });
    }

    /// Runs one change through the helper, showing its output, then reads the catalog again.
    fn job(self: &Rc<Self>, verb: &str, names: &[&str], reason: &str, success: &str) {
        if self.busy.replace(true) {
            return;
        }
        self.show_detail();
        self.update_all.set_sensitive(false);
        self.status.set_label("Working…");
        self.status.remove_css_class("error");
        self.status.set_visible(true);
        self.log.buffer().set_text("");
        self.log_box.set_visible(true);
        let mut args = vec![verb.to_owned()];
        args.extend(names.iter().map(|name| (*name).to_owned()));
        let (hub, success) = (self.clone(), success.to_owned());
        let output = self.clone();
        runner::run(
            Some(self.window.upcast_ref()),
            reason,
            args,
            move |text| {
                let buffer = output.log.buffer();
                buffer.insert(&mut buffer.end_iter(), &format!("{text}\n"));
                let mut end = buffer.end_iter();
                output.log.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
            },
            move |result| {
                hub.busy.set(false);
                match result {
                    Ok(()) => hub.status.set_label(&success),
                    Err(error) => {
                        hub.status.set_label(&error);
                        hub.status.add_css_class("error");
                    }
                }
                hub.status.set_visible(true);
                hub.reload();
            },
        );
    }
}
