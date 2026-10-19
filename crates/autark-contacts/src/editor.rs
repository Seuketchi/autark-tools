//! The contact editor: name, organisation, addresses and numbers (one a line) and notes.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::Stored;
use crate::logic;
use crate::window::Contacts;

fn field(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let name = gtk::Label::builder().label(label).xalign(0.0).build();
    name.add_css_class("field-label");
    row.append(&name);
    row.append(widget);
    row
}

fn lines(text: &[String]) -> gtk::ScrolledWindow {
    let view = gtk::TextView::builder().wrap_mode(gtk::WrapMode::Word).build();
    view.buffer().set_text(&text.join("\n"));
    gtk::ScrolledWindow::builder().child(&view).min_content_height(60).build()
}

fn text_of(scroller: &gtk::ScrolledWindow) -> String {
    let Some(view) = scroller.child().and_downcast::<gtk::TextView>() else { return String::new() };
    let buffer = view.buffer();
    buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
}

pub fn open(contacts: &Rc<Contacts>, existing: Option<Stored>) {
    let books = contacts.backend.snapshot();
    if books.is_empty() {
        return;
    }
    let contact = existing.as_ref().map(|stored| stored.contact.clone()).unwrap_or_default();
    let window = gtk::Window::builder().title(if existing.is_some() { "Edit contact" } else { "New contact" }).transient_for(&contacts.window).modal(true).default_width(420).resizable(false).build();
    window.add_css_class("editor");

    let name = gtk::Entry::builder().placeholder_text("Name").text(&contact.name).build();
    let organisation = gtk::Entry::builder().placeholder_text("Organisation").text(&contact.organisation).build();
    let emails = lines(&contact.emails);
    let phones = lines(&contact.phones);
    let notes = lines(&if contact.note.is_empty() { Vec::new() } else { vec![contact.note.clone()] });
    let labels: Vec<String> = books.iter().map(|book| format!("{} — {}", book.account_name, book.name)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let which = gtk::DropDown::from_strings(&refs);
    which.set_sensitive(existing.is_none());
    let error = gtk::Label::builder().xalign(0.0).wrap(true).build();
    error.add_css_class("status");
    error.add_css_class("error");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("primary");
    let cancel = gtk::Button::with_label("Cancel");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&save);
    let closing = window.clone();
    cancel.connect_clicked(move |_| closing.close());

    let form = gtk::Box::new(gtk::Orientation::Vertical, 12);
    form.set_margin_top(18);
    form.set_margin_bottom(18);
    form.set_margin_start(18);
    form.set_margin_end(18);
    form.append(&name);
    form.append(&organisation);
    form.append(&field("Email (one a line)", &emails));
    form.append(&field("Phone (one a line)", &phones));
    form.append(&field("Notes", &notes));
    if existing.is_none() {
        form.append(&field("Address book", &which));
    }
    form.append(&error);
    form.append(&buttons);
    window.set_child(Some(&form));

    let (weak, window_for_save) = (Rc::downgrade(contacts), window.clone());
    save.connect_clicked(move |button| {
        let Some(me) = weak.upgrade() else { return };
        let (email_text, phone_text) = (text_of(&emails), text_of(&phones));
        let uid = existing.as_ref().map(|stored| stored.contact.uid.clone()).unwrap_or_else(new_uid);
        let made = logic::build(&uid, &name.text(), &organisation.text(), &email_text, &phone_text, &text_of(&notes));
        if made.name.is_empty() && made.emails.is_empty() {
            error.set_text("Give the contact a name or an email address.");
            return;
        }
        if let Some(bad) = made.emails.iter().find(|email| !logic::valid_email(email)) {
            error.set_text(&format!("“{bad}” isn't an email address."));
            return;
        }
        let backend = me.backend.clone();
        let resource = existing.as_ref().map(|stored| stored.url.clone());
        let book = books[which.selected() as usize].clone();
        button.set_sensitive(false);
        let (window, error, button) = (window_for_save.clone(), error.clone(), button.clone());
        glib::spawn_future_local(async move {
            let result = blocking::unblock(move || match resource {
                Some(resource) => backend.update(&resource, made),
                None => backend.create(&book.account, &book.url, made),
            })
            .await;
            match result {
                Ok(()) => {
                    window.close();
                    me.draw();
                }
                Err(why) => {
                    error.set_text(&why);
                    button.set_sensitive(true);
                }
            }
        });
    });
    window.present();
}

/// An ID no other contact has: the time to the nanosecond and 64 random bits.
pub fn new_uid() -> String {
    let mut random = [0u8; 8];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut file, &mut random);
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
    format!("{nanos:x}-{:016x}@autark", u64::from_le_bytes(random))
}
