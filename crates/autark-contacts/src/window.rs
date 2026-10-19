//! The contacts window: the people on the left, searchable and grouped by letter, and the selected person on the right.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use autark_common::theme::rgb;
use gtk::glib;
use gtk::prelude::*;

use crate::backend::{Backend, Stored};
use crate::{editor, logic};

pub struct Contacts {
    pub window: gtk::ApplicationWindow,
    pub backend: Arc<Backend>,
    search: gtk::SearchEntry,
    list: gtk::ListBox,
    detail: gtk::Box,
    status: gtk::Label,
    /// What each list row stands for: the contact's address on the server, or `None` for a letter heading.
    rows: RefCell<Vec<Option<Stored>>>,
    selected: RefCell<Option<String>>,
    syncing: std::cell::Cell<bool>,
}

fn clear(container: &impl IsA<gtk::Widget>) {
    while let Some(child) = container.first_child() {
        child.unparent();
    }
}

/// A round picture with the contact's initials, in a colour chosen from their name.
fn avatar(initials: &str, seed: &str, size: i32) -> gtk::DrawingArea {
    const COLOURS: [&str; 6] = ["#5B8DEF", "#E07A5F", "#3D9A6E", "#B565A7", "#D9A23B", "#4A9DB5"];
    let hash = seed.bytes().fold(0usize, |hash, byte| hash.wrapping_mul(31).wrapping_add(usize::from(byte)));
    let [r, g, b] = rgb(COLOURS[hash % COLOURS.len()]);
    let initials = initials.to_owned();
    let area = gtk::DrawingArea::builder().content_width(size).content_height(size).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, width, height| {
        let (w, h) = (f64::from(width), f64::from(height));
        cr.set_source_rgb(r, g, b);
        cr.arc(w / 2.0, h / 2.0, w.min(h) / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.select_font_face("sans-serif", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Bold);
        cr.set_font_size(h * 0.4);
        if let Ok(extents) = cr.text_extents(&initials) {
            cr.move_to(w / 2.0 - extents.width() / 2.0 - extents.x_bearing(), h / 2.0 + extents.height() / 2.0);
            let _ = cr.show_text(&initials);
        }
    });
    area
}

impl Contacts {
    pub fn new(app: &gtk::Application, backend: Arc<Backend>) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Contacts").default_width(900).default_height(640).build();
        window.add_css_class("contacts");
        let header = gtk::HeaderBar::new();
        let new = gtk::Button::with_label("New contact");
        new.add_css_class("primary");
        header.pack_start(&new);
        window.set_titlebar(Some(&header));

        let search = gtk::SearchEntry::builder().placeholder_text("Search").build();
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
        status.add_css_class("status");
        let people = gtk::Box::new(gtk::Orientation::Vertical, 0);
        people.add_css_class("people");
        people.set_size_request(300, -1);
        people.append(&search);
        people.append(&scroller);
        people.append(&status);
        let detail = gtk::Box::new(gtk::Orientation::Vertical, 4);
        detail.add_css_class("detail");
        detail.set_hexpand(true);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&people);
        root.append(&detail);
        window.set_child(Some(&root));

        let me = Rc::new(Self { window, backend, search, list, detail, status, rows: RefCell::default(), selected: RefCell::default(), syncing: std::cell::Cell::new(false) });
        let weak = Rc::downgrade(&me);
        me.search.connect_search_changed({
            let weak = weak.clone();
            move |_| {
                if let Some(me) = weak.upgrade() {
                    me.draw();
                }
            }
        });
        me.list.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let (Some(me), Some(row)) = (weak.upgrade(), row) else { return };
                let stored = me.rows.borrow().get(row.index() as usize).cloned().flatten();
                *me.selected.borrow_mut() = stored.as_ref().map(|stored| stored.url.clone());
                me.draw_detail(stored.as_ref());
            }
        });
        new.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(me) = weak.upgrade() {
                    editor::open(&me, None);
                }
            }
        });
        me.draw();
        me.sync();
        me
    }

    /// Fetches every address book, then redraws.
    pub fn sync(self: &Rc<Self>) {
        if self.syncing.replace(true) {
            return;
        }
        let (backend, me) = (self.backend.clone(), Rc::downgrade(self));
        self.status.remove_css_class("error");
        self.status.set_text("Syncing…");
        glib::spawn_future_local(async move {
            let problems = blocking::unblock(move || backend.refresh()).await;
            let Some(me) = me.upgrade() else { return };
            me.syncing.set(false);
            if problems.is_empty() {
                let books = me.backend.snapshot();
                let total: usize = books.iter().map(|book| book.contacts.len()).sum();
                me.status.set_text(&if books.is_empty() { "No address books yet. Add an account: autark-accounts add-google".to_owned() } else { format!("{total} contact{}", if total == 1 { "" } else { "s" }) });
            } else {
                me.status.add_css_class("error");
                me.status.set_text(&problems.join("  ·  "));
            }
            me.draw();
        });
    }

    /// Everyone matching the search, in order.
    fn people(&self) -> Vec<Stored> {
        let query = self.search.text().to_string();
        let mut people: Vec<Stored> = self.backend.snapshot().into_iter().flat_map(|book| book.contacts).filter(|stored| logic::matches(&stored.contact, &query)).collect();
        people.sort_by_key(|stored| logic::sort_key(&stored.contact));
        people
    }

    pub fn draw(self: &Rc<Self>) {
        let people = self.people();
        let keep = self.selected.borrow().clone();
        self.list.remove_all();
        let mut rows: Vec<Option<Stored>> = Vec::new();
        let mut letter = None;
        let mut select = None;
        for stored in people {
            let section = logic::section(&stored.contact);
            if letter != Some(section) {
                letter = Some(section);
                let heading = gtk::Label::builder().label(section.to_string()).xalign(0.0).build();
                heading.add_css_class("letter");
                let row = gtk::ListBoxRow::builder().child(&heading).selectable(false).activatable(false).build();
                self.list.append(&row);
                rows.push(None);
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let title = logic::title(&stored.contact);
            line.append(&avatar(&logic::initials(&stored.contact), &title, 32));
            let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
            text.append(&gtk::Label::builder().label(&title).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build());
            let sub = stored.contact.emails.first().or(stored.contact.phones.first()).cloned().unwrap_or_else(|| stored.contact.organisation.clone());
            if !sub.is_empty() && sub != title {
                let sub = gtk::Label::builder().label(sub).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
                sub.add_css_class("sub");
                text.append(&sub);
            }
            line.append(&text);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            self.list.append(&row);
            if keep.as_deref() == Some(stored.url.as_str()) {
                select = Some(row);
            }
            rows.push(Some(stored));
        }
        *self.rows.borrow_mut() = rows;
        match select {
            Some(row) => self.list.select_row(Some(&row)),
            None => {
                *self.selected.borrow_mut() = None;
                self.draw_detail(None);
            }
        }
    }

    fn draw_detail(self: &Rc<Self>, stored: Option<&Stored>) {
        clear(&self.detail);
        let Some(stored) = stored else {
            let empty = gtk::Label::builder().label("Select a contact.").xalign(0.0).build();
            empty.add_css_class("empty");
            self.detail.append(&empty);
            return;
        };
        let contact = &stored.contact;
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        top.append(&avatar(&logic::initials(contact), &logic::title(contact), 72));
        let names = gtk::Box::new(gtk::Orientation::Vertical, 2);
        names.set_valign(gtk::Align::Center);
        let name = gtk::Label::builder().label(logic::title(contact)).xalign(0.0).wrap(true).selectable(true).build();
        name.add_css_class("name");
        names.append(&name);
        if !contact.organisation.is_empty() && contact.name.trim() != "" {
            let org = gtk::Label::builder().label(&contact.organisation).xalign(0.0).build();
            org.add_css_class("org");
            names.append(&org);
        }
        top.append(&names);
        self.detail.append(&top);
        let section = |title: &str, values: &[String]| {
            if values.is_empty() {
                return;
            }
            let label = gtk::Label::builder().label(title).xalign(0.0).build();
            label.add_css_class("field-label");
            self.detail.append(&label);
            for value in values {
                let text = gtk::Label::builder().label(value).xalign(0.0).selectable(true).wrap(true).build();
                text.add_css_class("value");
                self.detail.append(&text);
            }
        };
        section("Email", &contact.emails);
        section("Phone", &contact.phones);
        section("Notes", &if contact.note.is_empty() { Vec::new() } else { vec![contact.note.clone()] });
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_margin_top(24);
        let edit = gtk::Button::with_label("Edit");
        let delete = gtk::Button::with_label("Delete");
        delete.add_css_class("danger");
        buttons.append(&edit);
        buttons.append(&delete);
        self.detail.append(&buttons);
        let (weak, stored_for_edit) = (Rc::downgrade(self), stored.clone());
        edit.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                editor::open(&me, Some(stored_for_edit.clone()));
            }
        });
        let (weak, url) = (Rc::downgrade(self), stored.url.clone());
        delete.connect_clicked(move |button| {
            let Some(me) = weak.upgrade() else { return };
            button.set_sensitive(false);
            let (backend, url) = (me.backend.clone(), url.clone());
            glib::spawn_future_local(async move {
                match blocking::unblock(move || backend.delete(&url)).await {
                    Ok(()) => me.draw(),
                    Err(why) => {
                        me.status.add_css_class("error");
                        me.status.set_text(&why);
                    }
                }
            });
        });
    }
}
