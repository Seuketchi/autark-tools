//! Asking for your password and running the root helper (`autark-settings-helper`) through `sudo`, the way the
//! fingerprint-for-sudo switch always has. The password is kept in memory for five minutes after it's accepted, so a few
//! changes in a row ask once, and never written anywhere. The helper does only what its verbs say and checks every
//! argument; see `bin/autark-settings-helper.rs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use zeroize::Zeroizing;

use crate::ui;

pub const HELPER: &str = "/usr/libexec/autark/autark-settings-helper";
const REMEMBER: Duration = Duration::from_secs(300);
const SETUP: &str = "sudo isn't set up to run Settings' helper yet. Once, in a root shell: su -c 'sh ~/autark-tools/crates/autark-settings/data/install-helper.sh'";

thread_local! {
    static PASSWORD: RefCell<Option<(Zeroizing<String>, Instant)>> = const { RefCell::new(None) };
}

pub fn installed() -> bool {
    std::path::Path::new(HELPER).exists()
}

fn remembered() -> Option<Zeroizing<String>> {
    PASSWORD.with_borrow(|slot| slot.as_ref().filter(|(_, when)| when.elapsed() < REMEMBER).map(|(password, _)| password.clone()))
}

fn remember(password: &Zeroizing<String>) {
    PASSWORD.with_borrow_mut(|slot| *slot = Some((password.clone(), Instant::now())));
}

fn forget() {
    PASSWORD.with_borrow_mut(|slot| *slot = None);
}

/// Runs the helper with `args`; `more` is extra input for it (a new password), after the line sudo reads. `done` gets the
/// helper's output, or why it failed. Asks for the password unless one was accepted in the last five minutes.
pub fn run(parent: Option<&gtk::Window>, reason: &str, args: Vec<String>, more: Option<Zeroizing<String>>, done: impl FnOnce(Result<String, String>) + 'static) {
    if !installed() {
        done(Err(SETUP.to_owned()));
        return;
    }
    let done: Rc<RefCell<Option<Box<dyn FnOnce(Result<String, String>)>>>> = Rc::new(RefCell::new(Some(Box::new(done))));
    attempt(parent.cloned(), reason.to_owned(), args, more, done, remembered());
}

fn attempt(parent: Option<gtk::Window>, reason: String, args: Vec<String>, more: Option<Zeroizing<String>>, done: Rc<RefCell<Option<Box<dyn FnOnce(Result<String, String>)>>>>, known: Option<Zeroizing<String>>) {
    let finish = move |result: Result<String, String>| {
        if let Some(done) = done.borrow_mut().take() {
            done(result);
        }
    };
    match known {
        Some(password) => {
            glib::spawn_future_local(async move {
                match sudo(HELPER, args.clone(), password.clone(), more.clone()).await {
                    Ok(text) => {
                        remember(&password);
                        finish(Ok(text));
                    }
                    Err(Failure::Password) => {
                        // It changed, or the five minutes were a guess: ask again.
                        forget();
                        attempt(parent.clone(), reason, args, more, Rc::new(RefCell::new(Some(Box::new(finish)))), None);
                    }
                    Err(Failure::Other(text)) => finish(Err(text)),
                }
            });
        }
        None => {
            let again = (parent.clone(), reason.clone(), args.clone(), more.clone());
            let finish = Rc::new(RefCell::new(Some(finish)));
            ask_password(parent.as_ref(), &reason, move |password| {
                let Some(finish) = finish.borrow_mut().take() else { return };
                let Some(password) = password else {
                    finish(Err("Cancelled.".to_owned()));
                    return;
                };
                let (parent, reason, args, more) = again.clone();
                glib::spawn_future_local(async move {
                    match sudo(HELPER, args, password.clone(), more).await {
                        Ok(text) => {
                            remember(&password);
                            finish(Ok(text));
                        }
                        Err(Failure::Password) => {
                            let _ = (parent, reason);
                            finish(Err("That password isn’t right.".to_owned()));
                        }
                        Err(Failure::Other(text)) => finish(Err(text)),
                    }
                });
            });
        }
    }
}

/// Runs the helper for a control: `status` shows what went wrong (or hides), and `after` gets the helper's output on success.
pub fn apply(from: &impl IsA<gtk::Widget>, reason: &str, args: &[&str], more: Option<Zeroizing<String>>, status: &gtk::Label, after: impl FnOnce(String) + 'static) {
    let parent = from.root().and_downcast::<gtk::Window>();
    let status = status.clone();
    run(parent.as_ref(), reason, args.iter().map(|arg| (*arg).to_owned()).collect(), more, move |result| match result {
        Ok(text) => {
            status.set_visible(false);
            after(text);
        }
        Err(error) => {
            status.set_label(&error);
            status.set_visible(true);
        }
    });
}

pub enum Failure {
    Password,
    Other(String),
}

/// `sudo -S -k program args…`, the password on its first line of input and `more` on the next. The output is the
/// program's; on failure, the last thing it or sudo complained of.
pub async fn sudo(program: &str, args: Vec<String>, password: Zeroizing<String>, more: Option<Zeroizing<String>>) -> Result<String, Failure> {
    let (sender, receiver) = async_channel::bounded(1);
    let program = program.to_owned();
    std::thread::spawn(move || {
        use std::io::Write;
        let result = (|| {
            let mut child = std::process::Command::new("sudo")
                .args(["-S", "-k", "-p", "", &program])
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|error| Failure::Other(format!("sudo: {error}")))?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(format!("{}\n", password.as_str()).as_bytes());
                if let Some(more) = more {
                    let _ = stdin.write_all(format!("{}\n", more.as_str()).as_bytes());
                }
            }
            let output = child.wait_with_output().map_err(|error| Failure::Other(format!("sudo: {error}")))?;
            if output.status.success() {
                return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
            }
            let said = String::from_utf8_lossy(&output.stderr);
            if said.contains("incorrect password") || said.contains("try again") {
                return Err(Failure::Password);
            }
            if said.contains("expected host name") || said.contains("parse error") || said.contains("not in the sudoers") || said.contains("may not run sudo") {
                return Err(Failure::Other(format!("{SETUP} ({})", said.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim())));
            }
            let last = said.lines().rfind(|line| !line.trim().is_empty()).unwrap_or("").trim();
            Err(Failure::Other(last.trim_start_matches("autark-settings-helper: ").to_owned()))
        })();
        let _ = sender.send_blocking(result);
    });
    receiver.recv().await.unwrap_or_else(|_| Err(Failure::Other("sudo stopped unexpectedly.".to_owned())))
}

/// A small dialog for the password sudo needs; `answered` gets it, or nothing when cancelled.
pub fn ask_password(parent: Option<&gtk::Window>, reason: &str, answered: impl Fn(Option<Zeroizing<String>>) + 'static) {
    let window = gtk::Window::builder().title("Password").modal(true).resizable(false).default_width(340).build();
    window.set_transient_for(parent);
    // Settings' own app id, so the bar and dock count it as Settings.
    window.set_application(parent.and_then(|parent| parent.application()).as_ref());
    window.add_css_class("settings");
    window.add_css_class("dialog");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.add_css_class("page");
    let note = ui::note(&format!("Settings needs your password to {reason}."));
    note.set_wrap(true);
    column.append(&note);
    let entry = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text("Password").build();
    entry.add_css_class("field");
    entry.update_property(&[gtk::accessible::Property::Label("Password")]);
    column.append(&entry);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let answered = Rc::new(RefCell::new(Some(Box::new(answered) as Box<dyn Fn(Option<Zeroizing<String>>)>)));
    let closing = Rc::new(Cell::new(false));
    let finish = {
        let (answered, window, closing) = (answered.clone(), window.downgrade(), closing.clone());
        move |password: Option<Zeroizing<String>>| {
            closing.set(true);
            if let Some(answered) = answered.borrow_mut().take() {
                answered(password);
            }
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    };
    let cancel = ui::button("Cancel", {
        let finish = finish.clone();
        move |_| finish(None)
    });
    let go = ui::button("Continue", {
        let (finish, entry) = (finish.clone(), entry.clone());
        move |_| finish(Some(Zeroizing::new(entry.text().to_string())))
    });
    go.add_css_class("suggested");
    entry.connect_activate({
        let finish = finish.clone();
        move |entry| finish(Some(Zeroizing::new(entry.text().to_string())))
    });
    // Closing the window counts as cancelling.
    window.connect_close_request(move |_| {
        if let Some(answered) = answered.borrow_mut().take() {
            answered(None);
        }
        glib::Propagation::Proceed
    });
    buttons.append(&cancel);
    buttons.append(&go);
    column.append(&buttons);
    window.set_child(Some(&column));
    window.present();
}
