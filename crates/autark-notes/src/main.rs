//! Notes: Apple Notes on the surface, Obsidian underneath. Every note is a Markdown file in a
//! folder you choose (the vault), linked with `[[note name]]`.
//!
//! `autark-notes [folder]` opens that folder; otherwise `vault` from notes.toml, the last vault
//! opened, or `~/Notes`.

mod config;
mod editor;
mod markdown;
mod state;
mod vault;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

fn main() -> glib::ExitCode {
    let arg = std::env::args().nth(1).map(PathBuf::from);
    let app = gtk::Application::builder().application_id("org.autark.Notes").build();
    app.connect_startup(|_| load_css());
    let notes: RefCell<Option<Rc<window::Notes>>> = RefCell::default();
    app.connect_activate(move |app| {
        if let Some(notes) = &*notes.borrow() {
            notes.window.present();
            return;
        }
        let root = vault_root(arg.clone());
        if let Err(err) = std::fs::create_dir_all(&root) {
            eprintln!("notes: {}: {err}", root.display());
        }
        config::remember_vault(&root);
        let window = window::Notes::new(app, &root);
        window.window.present();
        notes.replace(Some(window));
    });
    // GTK would read our folder argument as a file to open.
    app.run_with_args(&[] as &[&str])
}

fn vault_root(arg: Option<PathBuf>) -> PathBuf {
    let config: config::Notes = autark_common::config::load("notes");
    let root = arg
        .or_else(|| config.vault.as_deref().map(config::expand))
        .or_else(config::last_vault)
        .unwrap_or_else(|| config::home().join("Notes"));
    std::path::absolute(&root).unwrap_or(root)
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!("{}{}", autark_common::theme::LINEN.gtk_css(), include_str!("notes.css")));
    gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("no display"), &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}
