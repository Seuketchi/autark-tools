//! Lock screen using `ext-session-lock` and PAM.
//!
//! `autark-lock` locks every monitor now and exits once the right password is typed.
//! `autark-lock --daemon` stays running and locks on logind's request and before sleep.
//! If this process dies while locked, the compositor keeps the screen locked.

mod auth;
mod config;
mod daemon;
mod fingerprint;
mod notifications;

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use chrono::Local;
use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4_session_lock::Instance;
use zeroize::Zeroizing;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Power {
    fn reboot(&self, interactive: bool) -> zbus::Result<()>;
    fn power_off(&self, interactive: bool) -> zbus::Result<()>;
}

fn main() -> glib::ExitCode {
    let config: config::Lock = autark_common::config::load("lock");
    if std::env::args().any(|arg| arg == "--daemon") {
        if let Err(err) = glib::MainContext::default().block_on(daemon::run(config.lock_on_sleep)) {
            eprintln!("lock: {err}");
            return glib::ExitCode::FAILURE;
        }
        return glib::ExitCode::SUCCESS;
    }
    // Locking with no PAM service would leave no way back in.
    if !Path::new("/etc/pam.d").join(&config.pam_service).exists() {
        eprintln!("lock: not locking: /etc/pam.d/{} doesn't exist, so no password could unlock", config.pam_service);
        return glib::ExitCode::FAILURE;
    }

    let app = gtk::Application::builder()
        .application_id("org.autark.Lock")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| load_css());
    let config = Rc::new(config);
    app.connect_activate(move |app| start(app, config.clone()));
    app.run_with_args(&[] as &[&str])
}

/// Who is unlocking: the login name, and how to greet them.
struct User {
    name: String,
    display_name: String,
    avatar: Option<String>,
}

impl User {
    fn current() -> Self {
        let name = std::env::var("USER").unwrap_or_default();
        // The comment field of /etc/passwd holds the full name, before any commas.
        let display_name = std::fs::read_to_string("/etc/passwd")
            .unwrap_or_default()
            .lines()
            .map(|line| line.split(':').collect::<Vec<_>>())
            .find(|fields| fields.first() == Some(&name.as_str()))
            .and_then(|fields| fields.get(4).map(|gecos| gecos.split(',').next().unwrap_or("").to_owned()))
            .filter(|full| !full.is_empty())
            .unwrap_or_else(|| name.clone());
        let avatar = Some(format!("/var/lib/AccountsService/icons/{name}")).filter(|path| Path::new(path).is_file());
        Self { name, display_name, avatar }
    }
}

struct Lock {
    instance: Instance,
    config: Rc<config::Lock>,
    user: User,
    /// A password check is running; further attempts wait for it.
    checking: Cell<bool>,
    /// Every monitor's hint line, and what it says when nothing is happening.
    hints: RefCell<Vec<gtk::Label>>,
    idle_hint: RefCell<&'static str>,
    /// Every monitor's notification list, and what they show.
    notifications: RefCell<Vec<gtk::Box>>,
    groups: RefCell<Vec<notifications::Group>>,
}

const PASSWORD_HINT: &str = "Type your password to unlock";
const FINGERPRINT_HINT: &str = "Touch the fingerprint sensor or type your password";

impl Lock {
    fn hint(&self, text: &str) {
        for hint in self.hints.borrow().iter() {
            hint.set_label(text);
        }
    }
}

fn start(app: &gtk::Application, config: Rc<config::Lock>) {
    let hold = app.hold();
    let instance = Instance::new();
    instance.connect_locked(|_| {
        // `--daemon` waits for this line before letting the laptop sleep.
        println!("locked");
        let _ = std::io::stdout().flush();
    });
    instance.connect_failed({
        let app = app.clone();
        move |_| {
            eprintln!("lock: the compositor refused to lock (is another lock screen running?)");
            app.quit();
        }
    });
    instance.connect_unlocked({
        let app = app.clone();
        move |_| {
            let _ = &hold;
            app.quit();
        }
    });
    let lock = Rc::new(Lock {
        instance: instance.clone(),
        config,
        user: User::current(),
        checking: Cell::new(false),
        hints: RefCell::default(),
        idle_hint: RefCell::new(PASSWORD_HINT),
        notifications: RefCell::default(),
        groups: RefCell::default(),
    });
    instance.connect_monitor({
        let (app, lock) = (app.clone(), lock.clone());
        move |instance, monitor| {
            let window = window(&app, &lock);
            instance.assign_window_to_monitor(&window, monitor);
        }
    });
    if !instance.lock() {
        eprintln!("lock: the compositor doesn't support ext-session-lock");
        app.quit();
        return;
    }
    glib::spawn_future_local({
        let lock = lock.clone();
        async move {
            let result = notifications::watch(|groups| {
                for list in lock.notifications.borrow().iter() {
                    notifications::fill(list, &groups, lock.config.notification_text);
                }
                *lock.groups.borrow_mut() = groups;
            })
            .await;
            if let Err(err) = result {
                eprintln!("lock: notifications: {err}");
            }
        }
    });
    if lock.config.fingerprint {
        glib::spawn_future_local(async move {
            let user = lock.user.name.clone();
            let result = fingerprint::watch(&user, |event| match event {
                fingerprint::Event::Ready => {
                    *lock.idle_hint.borrow_mut() = FINGERPRINT_HINT;
                    lock.hint(FINGERPRINT_HINT);
                }
                fingerprint::Event::Matched => lock.instance.unlock(),
                fingerprint::Event::NotRecognised => lock.hint("Fingerprint not recognised. Try again"),
                fingerprint::Event::GaveUp => {
                    *lock.idle_hint.borrow_mut() = PASSWORD_HINT;
                    lock.hint("Fingerprint unavailable. Type your password");
                }
            })
            .await;
            if let Err(err) = result {
                eprintln!("lock: fingerprint: {err}");
            }
        });
    }
}

/// One monitor's lock screen: date and time, notifications, the user, the password field, and power buttons.
fn window(app: &gtk::Application, lock: &Rc<Lock>) -> gtk::ApplicationWindow {
    let date = gtk::Label::new(None);
    date.add_css_class("date");
    let time = gtk::Label::new(None);
    time.add_css_class("time");
    tick(date.downgrade(), time.downgrade());
    let clock = gtk::Box::new(gtk::Orientation::Vertical, 6);
    clock.add_css_class("clock");
    clock.set_halign(gtk::Align::Center);
    clock.append(&date);
    clock.append(&time);
    let notifications = gtk::Box::new(gtk::Orientation::Vertical, 8);
    notifications.add_css_class("notifications");
    notifications.set_halign(gtk::Align::Center);
    notifications::fill(&notifications, &lock.groups.borrow(), lock.config.notification_text);
    lock.notifications.borrow_mut().push(notifications.clone());
    // Between the clock and the form, taking only the room left: on a short screen the cards that
    // don't fit are left out rather than drawn over the form.
    let between = gtk::ScrolledWindow::builder()
        .child(&notifications)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_height(true)
        .vexpand(true)
        .valign(gtk::Align::Start)
        .build();
    notifications::fit(&between, &notifications);

    let avatar: gtk::Widget = match &lock.user.avatar {
        Some(path) => {
            let picture = gtk::Picture::for_filename(path);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_size_request(72, 72);
            // Clips the photo to the round avatar.
            picture.set_overflow(gtk::Overflow::Hidden);
            picture.upcast()
        }
        None => {
            let initial = lock.user.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
            gtk::Label::new(Some(&initial)).upcast()
        }
    };
    avatar.add_css_class("avatar");
    avatar.set_halign(gtk::Align::Center);
    let name = gtk::Label::new(Some(&lock.user.display_name));
    name.add_css_class("name");

    let entry = gtk::PasswordEntry::builder().placeholder_text("Password").hexpand(true).build();
    let submit = gtk::Button::from_icon_name("go-next-symbolic");
    submit.add_css_class("submit");
    submit.set_tooltip_text(Some("Unlock"));
    let field = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    field.add_css_class("field");
    field.set_halign(gtk::Align::Center);
    field.append(&entry);
    field.append(&submit);
    let hint = gtk::Label::new(Some(*lock.idle_hint.borrow()));
    hint.add_css_class("hint");
    lock.hints.borrow_mut().push(hint.clone());

    let form = gtk::Box::new(gtk::Orientation::Vertical, 14);
    form.add_css_class("form");
    form.set_halign(gtk::Align::Center);
    form.append(&avatar);
    form.append(&name);
    form.append(&field);
    form.append(&hint);

    let power = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    power.add_css_class("power");
    power.set_halign(gtk::Align::End);
    power.set_valign(gtk::Align::End);
    for (icon, label, restart) in [("system-reboot-symbolic", "Restart", true), ("system-shutdown-symbolic", "Shut down", false)] {
        let button = gtk::Button::from_icon_name(icon);
        button.set_tooltip_text(Some(label));
        button.connect_clicked(move |_| {
            glib::spawn_future_local(async move {
                let result = async {
                    let power = PowerProxy::new(&zbus::Connection::system().await?).await?;
                    if restart { power.reboot(false).await } else { power.power_off(false).await }
                };
                if let Err(err) = result.await {
                    eprintln!("lock: {}: {err}", if restart { "restarting" } else { "shutting down" });
                }
            });
        });
        power.append(&button);
    }

    let overlay = gtk::Overlay::new();
    overlay.add_css_class("lock");
    let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    backdrop.add_css_class("backdrop");
    overlay.set_child(Some(&backdrop));
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&clock);
    column.append(&between);
    column.append(&form);
    overlay.add_overlay(&column);
    overlay.add_overlay(&power);

    let unlock = {
        let (lock, entry, hint, field) = (lock.clone(), entry.clone(), hint.clone(), field.clone());
        move || {
            if lock.checking.replace(true) {
                return;
            }
            let password = Zeroizing::new(entry.text().to_string());
            entry.set_text("");
            entry.set_sensitive(false);
            hint.set_label("Checking…");
            let (lock, entry, hint, field) = (lock.clone(), entry.clone(), hint.clone(), field.clone());
            glib::spawn_future_local(async move {
                let user = lock.user.name.clone();
                if auth::check(lock.config.pam_service.clone(), user, password).await {
                    lock.instance.unlock();
                    return;
                }
                hint.set_label("Wrong password. Try again");
                field.add_css_class("error");
                entry.set_sensitive(true);
                entry.grab_focus();
                lock.checking.set(false);
                glib::timeout_add_local_once(Duration::from_millis(600), move || field.remove_css_class("error"));
            });
        }
    };
    let unlock = Rc::new(unlock);
    entry.connect_activate({
        let unlock = unlock.clone();
        move |_| unlock()
    });
    submit.connect_clicked(move |_| unlock());

    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("lock-window");
    window.set_child(Some(&overlay));
    window.connect_map(move |_| {
        entry.grab_focus();
    });
    window
}

/// Updates the date and time on the minute while the labels exist.
fn tick(date: glib::WeakRef<gtk::Label>, time: glib::WeakRef<gtk::Label>) {
    let (Some(date_label), Some(time_label)) = (date.upgrade(), time.upgrade()) else {
        return;
    };
    let now = Local::now();
    date_label.set_label(&now.format("%A, %-d %B").to_string());
    time_label.set_label(&now.format("%H:%M").to_string());
    let wait = Duration::from_secs((60 - now.timestamp() % 60) as u64)
        .saturating_sub(Duration::from_nanos(now.timestamp_subsec_nanos().into()));
    glib::timeout_add_local_once(wait, move || tick(date, time));
}

fn load_css() {
    autark_common::theme::install(include_str!("lock.css"));
}
