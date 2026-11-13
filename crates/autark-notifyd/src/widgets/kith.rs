//! Kith's widget: its newest suggestion, under its face. Suggestions are `autark-ai`'s, read again whenever they
//! change, and answered through it.

use autark_common::visor::{State, Visor};
use gtk::prelude::*;

use crate::shared::Shared;

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
pub trait Ai {
    fn open_suggestions(&self) -> zbus::Result<String>;
    fn answer_suggestion(&self, entry: u64, answer: &str) -> zbus::Result<String>;
    #[zbus(property(emits_changed_signal = "false"))]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(signal)]
    fn state_changed(&self, state: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn suggestions_changed(&self) -> zbus::Result<()>;
}

/// Kith's newest open suggestion, as `autark-ai` keeps it.
#[derive(Clone, PartialEq, Debug)]
pub struct Suggestion {
    pub entry: u64,
    pub text: String,
    /// The offer's button, empty when it offers nothing.
    pub offer: String,
}

/// The newest of `OpenSuggestions`' entries.
pub fn newest(json: &str) -> Option<Suggestion> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let entry = entries.last()?;
    Some(Suggestion {
        entry: entry["id"].as_u64()?,
        text: entry["text"].as_str().unwrap_or_default().to_owned(),
        offer: if entry["offer"].is_null() { String::new() } else { entry["offer_label"].as_str().unwrap_or_default().to_owned() },
    })
}

#[derive(Clone, Copy)]
pub enum Answer {
    Accept,
    NotNow,
    Never,
}

impl Answer {
    pub fn word(self) -> &'static str {
        match self {
            Answer::Accept => "accept",
            Answer::NotNow => "not_now",
            Answer::Never => "never",
        }
    }
}

pub fn view(suggestion: &Shared<Option<Suggestion>>, kith: &Shared<State>, on_answer: impl Fn(Answer) + 'static) -> gtk::Box {
    let face = Visor::new(24);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(face.widget());
    header.append(&super::caption("KITH"));
    let none = gtk::Label::builder().label("No suggestions").xalign(0.0).build();
    none.add_css_class("detail");

    let text = gtk::Label::builder().xalign(0.0).wrap(true).build();
    text.add_css_class("suggestion-text");
    let offer = gtk::Button::new();
    offer.add_css_class("primary");
    let not_now = gtk::Button::with_label("Not now");
    not_now.add_css_class("secondary");
    let never = gtk::Button::builder().label("Don't suggest again").hexpand(true).halign(gtk::Align::End).build();
    never.add_css_class("quiet");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.append(&offer);
    buttons.append(&not_now);
    buttons.append(&never);
    let suggestion_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    suggestion_box.add_css_class("suggestion");
    suggestion_box.append(&text);
    suggestion_box.append(&buttons);

    let on_answer = std::rc::Rc::new(on_answer);
    for (button, answer) in [(&offer, Answer::Accept), (&not_now, Answer::NotNow), (&never, Answer::Never)] {
        let on_answer = on_answer.clone();
        button.connect_clicked(move |_| on_answer(answer));
    }

    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("card");
    card.add_css_class("widget");
    card.append(&header);
    card.append(&none);
    card.append(&suggestion_box);

    kith.watch(&card, move |_, state| face.set_state(*state));
    suggestion.watch(&card, move |_, suggestion| {
        suggestion_box.set_visible(suggestion.is_some());
        none.set_visible(suggestion.is_none());
        if let Some(suggestion) = suggestion {
            text.set_label(&suggestion.text);
            offer.set_label(&suggestion.offer);
            offer.set_visible(!suggestion.offer.is_empty());
        }
    });
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_open_suggestion() {
        let json = r#"[{"id":4,"kind":"suggestion","text":"Old.","offer":null,"offer_label":""},
                       {"id":9,"kind":"suggestion","text":"kitchend has kept a core busy for 14 minutes. Quit it?","offer":{"action":"quit_app","app":"/x"},"offer_label":"Quit kitchend"}]"#;
        assert_eq!(newest(json), Some(Suggestion { entry: 9, text: "kitchend has kept a core busy for 14 minutes. Quit it?".into(), offer: "Quit kitchend".into() }));
        assert_eq!(newest("[]"), None);
    }
}
