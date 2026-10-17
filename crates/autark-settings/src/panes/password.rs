//! Change password: a dialog for the current password and the new one twice, then PAM's chauthtok on the
//! `passwd` service, on a thread of its own so the window keeps drawing. What PAM says is wrong (shadow's
//! rules, a mistyped current password) shows in the dialog.

use std::ffi::{CStr, CString};

use gtk::glib;
use gtk::prelude::*;
use pam_client::{Context, ConversationHandler, ErrorCode, Flag};
use zeroize::Zeroizing;

use crate::ui;

/// The PAM service that changes passwords.
const SERVICE: &str = "passwd";

pub fn open(parent: Option<&gtk::Window>) {
    let window = gtk::Window::builder().title("Change password").modal(true).resizable(false).default_width(380).build();
    window.set_transient_for(parent);
    // Settings' own app id, so the bar and dock count it as Settings.
    window.set_application(parent.and_then(|parent| parent.application()).as_ref());
    window.add_css_class("settings");
    window.add_css_class("dialog");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.add_css_class("page");
    let field = |label: &str| {
        let entry = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text(label).build();
        entry.add_css_class("field");
        entry.update_property(&[gtk::accessible::Property::Label(label)]);
        column.append(&entry);
        entry
    };
    let (current, new, again) = (field("Current password"), field("New password"), field("New password again"));
    let error = ui::note("");
    error.add_css_class("error");
    error.set_wrap(true);
    error.set_visible(false);
    column.append(&error);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", {
        let window = window.downgrade();
        move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    });
    let change = ui::button("Change password", |_| {});
    change.add_css_class("suggested");
    buttons.append(&cancel);
    buttons.append(&change);
    column.append(&buttons);
    window.set_child(Some(&column));

    let show = {
        let error = error.clone();
        move |text: &str| {
            error.set_label(text);
            error.set_visible(!text.is_empty());
        }
    };
    let submit = {
        let (window, current, new, again, change) = (window.downgrade(), current.clone(), new.clone(), again.clone(), change.clone());
        move || {
            let (old, fresh, repeated) = (Zeroizing::new(current.text().to_string()), Zeroizing::new(new.text().to_string()), Zeroizing::new(again.text().to_string()));
            if let Err(problem) = check(&old, &fresh, &repeated) {
                show(problem);
                return;
            }
            show("");
            change.set_sensitive(false);
            let (window, change, show) = (window.clone(), change.clone(), show.clone());
            glib::spawn_future_local(async move {
                let result = chauthtok(glib::user_name().to_string_lossy().into_owned(), old, fresh).await;
                change.set_sensitive(true);
                match result {
                    Ok(()) => {
                        if let Some(window) = window.upgrade() {
                            window.close();
                        }
                    }
                    Err(problem) => show(&problem),
                }
            });
        }
    };
    change.connect_clicked({
        let submit = submit.clone();
        move |_| submit()
    });
    again.connect_activate(move |_| submit());
    window.present();
}

/// What's wrong before PAM is asked, if anything.
fn check(current: &str, new: &str, again: &str) -> Result<(), &'static str> {
    match () {
        _ if current.is_empty() => Err("Type your current password."),
        _ if new.is_empty() => Err("Type a new password."),
        _ if new != again => Err("The new passwords don’t match."),
        _ if new == current => Err("The new password is the same as the current one."),
        _ => Ok(()),
    }
}

/// Answers PAM's questions: the current password first, the new one after, and keeps what it complains of.
struct Change {
    user: String,
    current: Zeroizing<String>,
    new: Zeroizing<String>,
    asked: usize,
    errors: Vec<String>,
}

impl Change {
    fn answer(&mut self, prompt: &str) -> &str {
        let prompt = prompt.to_lowercase();
        // pam_unix asks "Current password:", then "New password:" and "Retype new password:".
        let current = prompt.contains("current") || (self.asked == 0 && !prompt.contains("new"));
        self.asked += 1;
        if current { &self.current } else { &self.new }
    }
}

impl ConversationHandler for Change {
    fn prompt_echo_on(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        CString::new(self.user.as_str()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, prompt: &CStr) -> Result<CString, ErrorCode> {
        let answer = self.answer(&prompt.to_string_lossy()).to_owned();
        CString::new(answer).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, _message: &CStr) {}

    fn error_msg(&mut self, message: &CStr) {
        self.errors.push(message.to_string_lossy().trim().to_owned());
    }
}

async fn chauthtok(user: String, current: Zeroizing<String>, new: Zeroizing<String>) -> Result<(), String> {
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let conversation = Change { user: user.clone(), current, new, asked: 0, errors: Vec::new() };
        let result = match Context::new(SERVICE, Some(&user), conversation) {
            Ok(mut context) => {
                let changed = context.chauthtok(Flag::NONE);
                let errors = std::mem::take(&mut context.conversation_mut().errors);
                changed.map_err(|error| errors.last().cloned().unwrap_or_else(|| error.to_string()))
            }
            Err(error) => Err(error.to_string()),
        };
        let _ = sender.send_blocking(result);
    });
    receiver.recv().await.unwrap_or_else(|_| Err("The password change stopped unexpectedly.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_get_the_right_password() {
        let mut change = Change { user: "tan".into(), current: Zeroizing::new("old".into()), new: Zeroizing::new("New1pass".into()), asked: 0, errors: Vec::new() };
        assert_eq!(change.answer("Current password: "), "old");
        assert_eq!(change.answer("New password: "), "New1pass");
        assert_eq!(change.answer("Retype new password: "), "New1pass");
        let mut change = Change { asked: 0, ..change };
        assert_eq!(change.answer("Password: "), "old", "an unlabelled first question is the current one");
    }

    #[test]
    fn mistakes_are_caught_before_pam() {
        assert!(check("", "a", "a").is_err());
        assert!(check("old", "New1", "New2").is_err());
        assert!(check("same", "same", "same").is_err());
        assert!(check("old", "New1pass", "New1pass").is_ok());
    }
}
