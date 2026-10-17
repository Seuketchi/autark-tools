//! The focused app's name, from the compositor's window list. Hidden when nothing has focus or
//! the compositor lacks the protocol.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;

use crate::shared::Shared;

/// Watches the compositor once for every bar; `None` hides the name.
pub fn source() -> Shared<Option<String>> {
    let shared = Shared::new(None);
    let names = RefCell::new(HashMap::new());
    let watched = shared.clone();
    let result = autark_common::toplevels::watch(move |windows| {
        let focused = windows.iter().find(|window| window.activated && !window.app_id.is_empty());
        watched.set(focused.map(|window| {
            names
                .borrow_mut()
                .entry(window.app_id.clone())
                .or_insert_with(|| {
                    autark_common::desktop::app_name(&window.app_id).unwrap_or_else(|| fallback_name(&window.app_id))
                })
                .clone()
        }));
    });
    if let Err(err) = result {
        eprintln!("app name: {err}");
    }
    shared
}

pub fn view(source: &Shared<Option<String>>) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("item");
    label.add_css_class("app-name");
    source.watch(&label, |label, name| {
        label.set_visible(name.is_some());
        label.set_label(name.as_deref().unwrap_or_default());
    });
    label
}

/// "org.example.TextEditor" reads as "TextEditor", "foot" as "Foot".
fn fallback_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
