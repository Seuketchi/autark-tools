//! The calculator's window: the sum you're typing, its answer underneath as it stands, and a keypad.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::calc;

/// The keys, row by row: what's shown, what it does, and how it looks.
const KEYS: [[(&str, Key, &str); 4]; 6] = [
    [("AC", Key::Clear, "function"), ("(", Key::Text("("), "function"), (")", Key::Text(")"), "function"), ("⌫", Key::Back, "function")],
    [("7", Key::Text("7"), "digit"), ("8", Key::Text("8"), "digit"), ("9", Key::Text("9"), "digit"), ("÷", Key::Text("÷"), "operator")],
    [("4", Key::Text("4"), "digit"), ("5", Key::Text("5"), "digit"), ("6", Key::Text("6"), "digit"), ("×", Key::Text("×"), "operator")],
    [("1", Key::Text("1"), "digit"), ("2", Key::Text("2"), "digit"), ("3", Key::Text("3"), "digit"), ("−", Key::Text("−"), "operator")],
    [("0", Key::Text("0"), "digit"), (".", Key::Text("."), "digit"), ("%", Key::Text("%"), "function"), ("+", Key::Text("+"), "operator")],
    [("=", Key::Equals, "equals"), ("", Key::Equals, ""), ("", Key::Equals, ""), ("", Key::Equals, "")],
];

#[derive(Clone, Copy)]
enum Key {
    Text(&'static str),
    Clear,
    Back,
    Equals,
}

pub struct Calc {
    pub window: gtk::ApplicationWindow,
    entry: gtk::Entry,
    result: gtk::Label,
    history: gtk::Label,
    /// The answer as it stands, for copying.
    answer: RefCell<Option<String>>,
}

impl Calc {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Calculator").default_width(340).default_height(540).resizable(false).build();
        window.add_css_class("calc");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let controls = autark_common::chrome::controls(false);
        let handle = gtk::WindowHandle::builder().child(&controls).build();
        handle.add_css_class("handle");

        let history = gtk::Label::builder().xalign(1.0).ellipsize(pango::EllipsizeMode::Start).build();
        history.add_css_class("history");
        let entry = gtk::Entry::builder().xalign(1.0).placeholder_text("0").hexpand(true).build();
        entry.add_css_class("sum");
        let result = gtk::Label::builder().xalign(1.0).ellipsize(pango::EllipsizeMode::Start).selectable(false).build();
        result.add_css_class("answer");
        let display = gtk::Box::new(gtk::Orientation::Vertical, 2);
        display.add_css_class("display");
        display.append(&history);
        display.append(&entry);
        display.append(&result);

        let pad = gtk::Grid::builder().row_spacing(8).column_spacing(8).row_homogeneous(true).column_homogeneous(true).vexpand(true).build();
        pad.add_css_class("pad");
        let calc = Rc::new(Self { window, entry, result, history, answer: RefCell::default() });
        for (row, keys) in KEYS.iter().enumerate() {
            for (column, (label, key, style)) in keys.iter().enumerate() {
                // The last row is one wide equals key.
                if row == KEYS.len() - 1 && column > 0 {
                    continue;
                }
                let button = gtk::Button::with_label(label);
                button.add_css_class("key");
                button.add_css_class(style);
                button.set_focus_on_click(false);
                let (me, key) = (Rc::downgrade(&calc), *key);
                button.connect_clicked(move |_| {
                    if let Some(me) = me.upgrade() {
                        me.press(key);
                    }
                });
                let span = if row == KEYS.len() - 1 { 4 } else { 1 };
                pad.attach(&button, column as i32, row as i32, span, 1);
            }
        }

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&handle);
        content.append(&display);
        content.append(&pad);
        calc.window.set_child(Some(&content));

        let me = Rc::downgrade(&calc);
        calc.entry.connect_changed({
            let me = me.clone();
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.preview();
                }
            }
        });
        calc.entry.connect_activate({
            let me = me.clone();
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.press(Key::Equals);
                }
            }
        });

        // Esc clears; the keys the field doesn't take go to it anyway.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let me = me.clone();
            move |_, key, _, _| match (me.upgrade(), key) {
                (Some(me), gdk::Key::Escape) => {
                    me.press(Key::Clear);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        calc.window.add_controller(keys);

        // Clicking the answer copies it.
        let copy = gtk::GestureClick::new();
        copy.connect_released({
            let me = me.clone();
            move |_, _, _, _| {
                if let Some(me) = me.upgrade()
                    && let Some(answer) = me.answer.borrow().as_deref()
                {
                    me.window.clipboard().set_text(&answer.replace(',', ""));
                }
            }
        });
        calc.result.add_controller(copy);
        calc.result.set_tooltip_text(Some("Click to copy"));

        calc.entry.grab_focus();
        let keep = calc.clone();
        calc.window.connect_close_request(move |_| {
            let _ = &keep;
            glib::Propagation::Proceed
        });
        calc
    }

    fn press(&self, key: Key) {
        match key {
            Key::Text(text) => {
                let mut position = self.entry.position();
                self.entry.insert_text(text, &mut position);
                self.entry.set_position(position);
            }
            Key::Clear => {
                self.entry.set_text("");
                self.history.set_text("");
            }
            Key::Back => {
                let position = self.entry.position();
                if self.entry.selection_bounds().is_some() {
                    self.entry.delete_selection();
                } else if position > 0 {
                    self.entry.delete_text(position - 1, position);
                }
            }
            Key::Equals => match calc::eval(&self.entry.text()) {
                Ok(value) => {
                    let sum = self.entry.text().to_string();
                    let shown = calc::show(value);
                    self.history.set_text(&format!("{sum} ="));
                    self.entry.set_text(&shown.replace(',', ""));
                    self.entry.set_position(-1);
                }
                Err(error) => self.result.set_text(error.message()),
            },
        }
        self.entry.grab_focus_without_selecting();
    }

    /// The answer as the sum stands, so you see it before you ask.
    fn preview(&self) {
        let text = self.entry.text();
        // A lone number is its own answer, which says nothing.
        let shown = match calc::eval(&text) {
            Ok(value) if !text.trim().is_empty() && !is_plain_number(&text) => Some(calc::show(value)),
            _ => None,
        };
        self.result.set_text(shown.as_deref().unwrap_or(""));
        *self.answer.borrow_mut() = shown;
    }
}

/// Just a number, with nothing to work out.
fn is_plain_number(text: &str) -> bool {
    let text = text.trim().trim_start_matches(['-', '−']);
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

#[cfg(test)]
mod tests {
    use super::is_plain_number;

    #[test]
    fn a_number_alone_needs_no_answer() {
        assert!(is_plain_number("42"));
        assert!(is_plain_number("-3.5"));
        assert!(is_plain_number("1,000"));
        assert!(!is_plain_number("2+2"));
        assert!(!is_plain_number(""));
        assert!(!is_plain_number("50%"));
    }
}
