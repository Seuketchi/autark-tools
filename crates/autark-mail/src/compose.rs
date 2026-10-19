//! The compose window: who it's from, who it's to, a subject and the text.

use std::rc::Rc;

use autark_pim::mime::{Address, Outgoing};
use gtk::glib;
use gtk::prelude::*;

use crate::window::Mail;
use crate::{backend, logic};

/// What a compose window starts with.
#[derive(Default, Clone)]
pub struct Draft {
    /// Which account to send from, as an index into the account list.
    pub account: usize,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub in_reply_to: String,
}

fn field(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let name = gtk::Label::builder().label(label).xalign(0.0).build();
    name.add_css_class("field-label");
    row.append(&name);
    row.append(widget);
    row
}

pub fn open(mail: &Rc<Mail>, draft: Draft) {
    let accounts = mail.accounts();
    if accounts.is_empty() {
        return;
    }
    let window = gtk::Window::builder().title("New message").transient_for(&mail.window).default_width(620).default_height(520).build();
    window.add_css_class("compose");
    let names: Vec<String> = accounts.iter().map(|account| format!("{} <{}>", account.name, account.email)).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let from = gtk::DropDown::from_strings(&refs);
    from.set_selected(draft.account.min(accounts.len() - 1) as u32);
    let to = gtk::Entry::builder().text(&draft.to).build();
    let cc = gtk::Entry::builder().text(&draft.cc).build();
    let subject = gtk::Entry::builder().text(&draft.subject).build();
    let body = gtk::TextView::builder().wrap_mode(gtk::WrapMode::Word).left_margin(6).right_margin(6).top_margin(6).build();
    body.buffer().set_text(&draft.body);
    let scroller = gtk::ScrolledWindow::builder().child(&body).vexpand(true).min_content_height(220).build();
    let error = gtk::Label::builder().xalign(0.0).wrap(true).build();
    error.add_css_class("status");
    error.add_css_class("error");
    let send = gtk::Button::with_label("Send");
    send.add_css_class("primary");
    let cancel = gtk::Button::with_label("Discard");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&send);
    let closing = window.clone();
    cancel.connect_clicked(move |_| closing.close());

    let form = gtk::Box::new(gtk::Orientation::Vertical, 10);
    form.set_margin_top(16);
    form.set_margin_bottom(16);
    form.set_margin_start(16);
    form.set_margin_end(16);
    form.append(&field("From", &from));
    form.append(&field("To", &to));
    form.append(&field("Cc", &cc));
    form.append(&field("Subject", &subject));
    form.append(&scroller);
    form.append(&error);
    form.append(&buttons);
    window.set_child(Some(&form));

    let (weak, window_for_send) = (Rc::downgrade(mail), window.clone());
    send.connect_clicked(move |button| {
        let Some(me) = weak.upgrade() else { return };
        let account = accounts[from.selected() as usize].clone();
        let (to_list, cc_list) = (logic::addresses(&to.text()), logic::addresses(&cc.text()));
        if to_list.is_empty() {
            error.set_text("Add someone to send it to.");
            return;
        }
        let buffer = body.buffer();
        let message = Outgoing {
            from: Address { name: String::new(), email: account.email.clone() },
            to: to_list,
            cc: cc_list,
            subject: subject.text().trim().to_owned(),
            text: buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string(),
            in_reply_to: draft.in_reply_to.clone(),
            message_id: logic::message_id(&new_id(), &account.email),
            ..Default::default()
        };
        button.set_sensitive(false);
        error.set_text("Sending…");
        let (window, error, button) = (window_for_send.clone(), error.clone(), button.clone());
        glib::spawn_future_local(async move {
            match blocking::unblock(move || backend::send(&account, message)).await {
                Ok(()) => {
                    window.close();
                    me.say("Sent.", false);
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

/// A string no other message has: the time to the nanosecond and 64 random bits.
pub fn new_id() -> String {
    let mut random = [0u8; 8];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut file, &mut random);
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
    format!("{nanos:x}.{:016x}", u64::from_le_bytes(random))
}
