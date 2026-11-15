//! The passphrase dialog ssh-add opens (as SSH_ASKPASS) when Butler unlocks its key: one password field, and the
//! passphrase goes to ssh-add on stdout, never anywhere else. Cancel or close and ssh-add gets nothing.

use std::cell::Cell;
use std::io::Write;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

pub fn run(prompt: String) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.autark.Butler.Askpass")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let given = Rc::new(Cell::new(false));
    let done = Rc::clone(&given);
    app.connect_startup(|_| autark_common::theme::install(include_str!("butler.css")));
    app.connect_activate(move |app| {
        let window = gtk::ApplicationWindow::builder().application(app).title("Unlock Butler's key").modal(true).resizable(false).build();
        window.add_css_class("butler");
        let entry = gtk::PasswordEntry::builder().show_peek_icon(true).activates_default(true).build();
        let (ok, cancel) = (gtk::Button::with_label("Unlock"), gtk::Button::with_label("Cancel"));
        ok.add_css_class("suggested-action");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&ok);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        body.add_css_class("page");
        body.append(&gtk::Label::builder().label(prompt.trim()).wrap(true).xalign(0.0).build());
        body.append(&entry);
        body.append(&buttons);
        window.set_child(Some(&body));
        window.set_default_widget(Some(&ok));
        let (w, given) = (window.clone(), Rc::clone(&done));
        ok.connect_clicked(move |_| {
            let mut out = std::io::stdout();
            given.set(writeln!(out, "{}", entry.text()).and_then(|()| out.flush()).is_ok());
            w.close();
        });
        let w = window.clone();
        cancel.connect_clicked(move |_| w.close());
        window.present();
    });
    // The prompt is ssh-add's argument; GTK isn't given it to parse.
    app.run_with_args::<&str>(&[]);
    if given.get() { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE }
}
