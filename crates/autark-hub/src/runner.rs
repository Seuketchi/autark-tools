//! Running the root helper: `sudo -S -k -p "" autark-hub-helper <verb> <names>…`, with the password asked for in a small
//! dialog and kept in memory for five minutes (never written anywhere), the way Settings does it. The helper's output
//! comes back a line at a time, so a build shows as it happens.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use zeroize::Zeroizing;

pub const HELPER: &str = "/usr/libexec/autark/autark-hub-helper";
const REMEMBER: Duration = Duration::from_secs(300);
const SETUP: &str = "sudo isn't set up to run the Hub's helper yet. Once, in a root shell, from the autark-tools folder: su -c 'sh crates/autark-hub/data/install-helper.sh'";

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

/// The one call that reports how a job ended, taken the first time it is used.
type Done = Rc<RefCell<Option<Box<dyn FnOnce(Result<(), String>)>>>>;

enum Message {
    Line(String),
    Done(Result<(), Failure>),
}

enum Failure {
    Password,
    Other(String),
}

/// Runs `verb names…` as root. `line` gets each line of output; `done` gets the outcome (an error is the last thing the
/// helper, `autark-pkg` or sudo complained of). Asks for the password unless one was accepted in the last five minutes.
pub fn run(parent: Option<&gtk::Window>, reason: &str, args: Vec<String>, line: impl Fn(String) + 'static, done: impl FnOnce(Result<(), String>) + 'static) {
    if !installed() {
        done(Err(SETUP.to_owned()));
        return;
    }
    let line: Rc<dyn Fn(String)> = Rc::new(line);
    let done: Done = Rc::new(RefCell::new(Some(Box::new(done))));
    match remembered() {
        Some(password) => start(parent.cloned(), reason.to_owned(), args, password, line, done, true),
        None => ask(parent.cloned(), reason.to_owned(), args, line, done),
    }
}

fn ask(parent: Option<gtk::Window>, reason: String, args: Vec<String>, line: Rc<dyn Fn(String)>, done: Done) {
    let again = (parent.clone(), reason.clone(), args, line, done.clone());
    ask_password(parent.as_ref(), &reason, move |password| {
        let (parent, reason, args, line, done) = again.clone();
        match password {
            Some(password) => start(parent, reason, args, password, line, done, false),
            None => finish(&done, Err("Cancelled.".to_owned())),
        }
    });
}

fn finish(done: &Done, result: Result<(), String>) {
    if let Some(done) = done.borrow_mut().take() {
        done(result);
    }
}

/// `retry`: the password came from memory, so if sudo refuses it, ask for a fresh one instead of giving up.
fn start(parent: Option<gtk::Window>, reason: String, args: Vec<String>, password: Zeroizing<String>, line: Rc<dyn Fn(String)>, done: Done, retry: bool) {
    let (sender, receiver) = async_channel::unbounded();
    let secret = password.clone();
    let job = args.clone();
    std::thread::spawn(move || sudo(&job, &secret, &sender));
    glib::spawn_future_local(async move {
        while let Ok(message) = receiver.recv().await {
            match message {
                Message::Line(text) => line(text),
                Message::Done(Ok(())) => {
                    remember(&password);
                    finish(&done, Ok(()));
                    return;
                }
                Message::Done(Err(Failure::Password)) if retry => {
                    forget();
                    ask(parent, reason, args, line, done);
                    return;
                }
                Message::Done(Err(Failure::Password)) => {
                    forget();
                    finish(&done, Err("That password isn’t right.".to_owned()));
                    return;
                }
                Message::Done(Err(Failure::Other(text))) => {
                    finish(&done, Err(text));
                    return;
                }
            }
        }
        finish(&done, Err("sudo stopped unexpectedly.".to_owned()));
    });
}

/// On its own thread: the password goes in on sudo's first line of input, and both of its outputs are read to the end.
fn sudo(args: &[String], password: &Zeroizing<String>, sender: &async_channel::Sender<Message>) {
    let mut command = Command::new("sudo");
    command.args(["-S", "-k", "-p", "", HELPER]).args(args);
    run_command(command, password, sender);
}

/// Runs `command` (sudo and the helper, or a stand-in for them when testing) and sends its output and how it ended.
fn run_command(mut command: Command, password: &Zeroizing<String>, sender: &async_channel::Sender<Message>) {
    let result = (|| {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Failure::Other(format!("sudo: {error}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(format!("{}\n", password.as_str()).as_bytes());
        }
        let stdout = child.stdout.take().ok_or_else(|| Failure::Other("no output from sudo".to_owned()))?;
        let stderr = child.stderr.take().ok_or_else(|| Failure::Other("no output from sudo".to_owned()))?;
        let said = Rc::new(RefCell::new(Vec::<String>::new()));
        // stderr is read on a second thread so neither pipe fills up and stalls the build; its lines are kept for errors.
        let errors = {
            let sender = sender.clone();
            std::thread::spawn(move || {
                let mut kept = Vec::new();
                for text in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = sender.send_blocking(Message::Line(text.clone()));
                    kept.push(text);
                }
                kept
            })
        };
        for text in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = sender.send_blocking(Message::Line(text));
        }
        let kept = errors.join().unwrap_or_default();
        said.borrow_mut().extend(kept);
        let status = child.wait().map_err(|error| Failure::Other(format!("sudo: {error}")))?;
        if status.success() {
            return Ok(());
        }
        let said = said.borrow();
        if said.iter().any(|text| text.contains("incorrect password") || text.contains("try again")) {
            return Err(Failure::Password);
        }
        if said.iter().any(|text| text.contains("expected host name") || text.contains("parse error") || text.contains("not in the sudoers") || text.contains("may not run sudo")) {
            return Err(Failure::Other(SETUP.to_owned()));
        }
        let last = said.iter().rev().find(|text| !text.trim().is_empty()).map(|text| text.trim()).unwrap_or("it failed");
        Err(Failure::Other(last.trim_start_matches("autark-hub-helper: ").trim_start_matches("autark-pkg: ").to_owned()))
    })();
    let _ = sender.send_blocking(Message::Done(result));
}

/// A small dialog for the password sudo needs; `answered` gets it, or nothing when cancelled.
fn ask_password(parent: Option<&gtk::Window>, reason: &str, answered: impl Fn(Option<Zeroizing<String>>) + 'static) {
    let window = gtk::Window::builder().title("Password").modal(true).resizable(false).default_width(340).build();
    window.set_transient_for(parent);
    window.set_application(parent.and_then(|parent| parent.application()).as_ref());
    window.add_css_class("hub");
    window.add_css_class("dialog");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.add_css_class("dialog-body");
    let note = gtk::Label::builder().label(format!("Software needs your password to {reason}.")).wrap(true).xalign(0.0).build();
    note.add_css_class("note");
    column.append(&note);
    let entry = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text("Password").build();
    entry.add_css_class("field");
    entry.update_property(&[gtk::accessible::Property::Label("Password")]);
    column.append(&entry);

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
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("plain");
    cancel.connect_clicked({
        let finish = finish.clone();
        move |_| finish(None)
    });
    let go = gtk::Button::with_label("Continue");
    go.add_css_class("primary");
    let submit = {
        let (finish, entry) = (finish.clone(), entry.clone());
        move || {
            let text = entry.text();
            if !text.is_empty() {
                finish(Some(Zeroizing::new(text.to_string())));
            }
        }
    };
    go.connect_clicked({
        let submit = submit.clone();
        move |_| submit()
    });
    entry.connect_activate(move |_| submit());
    buttons.append(&cancel);
    buttons.append(&go);
    column.append(&buttons);
    window.set_child(Some(&column));
    // Closing the window any other way is a cancel, once.
    window.connect_close_request(move |_| {
        if !closing.get() {
            finish(None);
        }
        glib::Propagation::Proceed
    });
    window.present();
    entry.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Runs a shell script standing in for sudo, and returns what the runner sent: the lines, and how it ended. The script
    /// is run through `sh`, not executed itself, so a test thread forking at the wrong moment cannot make it "text file busy".
    fn run_fake(script: &str) -> (Vec<String>, Result<(), Failure>) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("autark-hub-runner-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sudo.sh");
        std::fs::write(&path, format!("read password\n{script}\n")).unwrap();
        let (sender, receiver) = async_channel::unbounded();
        let mut command = Command::new("sh");
        command.arg(&path).args(["install", "zlib"]);
        run_command(command, &Zeroizing::new("secret".to_owned()), &sender);
        let _ = std::fs::remove_dir_all(&dir);
        let (mut lines, mut end) = (Vec::new(), None);
        while let Ok(message) = receiver.try_recv() {
            match message {
                Message::Line(text) => lines.push(text),
                Message::Done(result) => end = Some(result),
            }
        }
        lines.sort();
        (lines, end.expect("a job always ends"))
    }

    #[test]
    fn output_from_both_pipes_comes_back_line_by_line_and_success_is_ok() {
        let (lines, end) = run_fake("echo building; echo \"a warning\" >&2; echo \"got $password\"");
        assert_eq!(lines, ["a warning", "building", "got secret"]);
        assert!(end.is_ok());
    }

    #[test]
    fn a_wrong_password_is_told_apart_from_other_failures() {
        let (_, end) = run_fake("echo \"sudo: 1 incorrect password attempt\" >&2; exit 1");
        assert!(matches!(end, Err(Failure::Password)));
    }

    #[test]
    fn a_refusal_from_autark_pkg_comes_back_as_its_last_message_without_the_program_prefix() {
        let (lines, end) = run_fake("echo \"removing zlib\"; echo \"autark-pkg: zlib is needed by openssl\" >&2; exit 1");
        assert_eq!(lines, ["autark-pkg: zlib is needed by openssl", "removing zlib"]);
        match end {
            Err(Failure::Other(text)) => assert_eq!(text, "zlib is needed by openssl"),
            other => panic!("expected the refusal, got {}", if other.is_ok() { "success" } else { "another failure" }),
        }
    }

    #[test]
    fn a_sudo_that_is_not_set_up_points_at_the_setup_script() {
        let (_, end) = run_fake("echo \"sudo: pam_authenticate: not in the sudoers file\" >&2; exit 1");
        match end {
            Err(Failure::Other(text)) => assert!(text.contains("install-helper.sh"), "{text}"),
            _ => panic!("expected the setup message"),
        }
    }
}
