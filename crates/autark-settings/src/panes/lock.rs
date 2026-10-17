//! Fingerprint & Lock: fingerprints through fprintd (listed, added with the sensor's stages shown, removed);
//! unlocking with one and showing notification text, which are `lock.toml`'s and read at each lock; using
//! one for sudo, written by `autark-fingerprint-sudo` through sudo; how soon it locks once the screens are
//! off, which is `power.toml`'s; and changing the password.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use autark_common::theme;
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;
use zeroize::Zeroizing;

use crate::{bus, config, icon, ui};

#[zbus::proxy(interface = "net.reactivated.Fprint.Manager", default_service = "net.reactivated.Fprint", default_path = "/net/reactivated/Fprint/Manager")]
trait Manager {
    fn get_default_device(&self) -> zbus::Result<zbus::OwnedObjectPath>;
}

#[zbus::proxy(interface = "net.reactivated.Fprint.Device", default_service = "net.reactivated.Fprint")]
trait Device {
    fn list_enrolled_fingers(&self, username: &str) -> zbus::Result<Vec<String>>;
    fn claim(&self, username: &str) -> zbus::Result<()>;
    fn release(&self) -> zbus::Result<()>;
    fn enroll_start(&self, finger_name: &str) -> zbus::Result<()>;
    fn enroll_stop(&self) -> zbus::Result<()>;
    fn delete_enrolled_finger(&self, finger_name: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn enroll_status(&self, result: &str, done: bool) -> zbus::Result<()>;
    #[zbus(property, name = "num-enroll-stages")]
    fn num_enroll_stages(&self) -> zbus::Result<i32>;
}

const FPRINTD: &str = "The fingerprint service (fprintd)";
/// The fingerprint glyph from the design.
const GLYPH: &str = "M7 11a5 5 0 0 1 10 0v2M12 11v6M9.5 21c-1-1.8-1.5-4-1.5-7M16.5 18c-.3 1.2-.8 2.3-1.4 3.2M4.5 15c-.3-1.3-.5-2.6-.5-4a8 8 0 0 1 14.5-4.6M20 11c0 1.2-.1 2.4-.3 3.5";
/// fprintd's finger names, in the order new ones are taken.
const FINGERS: [(&str, &str); 10] = [
    ("right-index-finger", "Right index finger"),
    ("left-index-finger", "Left index finger"),
    ("right-thumb", "Right thumb"),
    ("left-thumb", "Left thumb"),
    ("right-middle-finger", "Right middle finger"),
    ("left-middle-finger", "Left middle finger"),
    ("right-ring-finger", "Right ring finger"),
    ("left-ring-finger", "Left ring finger"),
    ("right-little-finger", "Right little finger"),
    ("left-little-finger", "Left little finger"),
];
const HELPER: &str = "/usr/libexec/autark/autark-fingerprint-sudo";
const SUDO_FILE: &str = "/etc/pam.d/autark-fingerprint";
const LOCK_AFTER: [(&str, &str); 4] = [("0", "Immediately"), ("60", "After 1 minute"), ("300", "After 5 minutes"), ("-1", "Never")];

struct Fingerprints {
    group: gtk::Box,
    status: gtk::Label,
    device: RefCell<Option<DeviceProxy<'static>>>,
    user: String,
    /// The enrolment in progress: its row, the stages done of how many, and its ring.
    enrolling: RefCell<Option<(gtk::Box, gtk::Label, gtk::DrawingArea)>>,
    stage: Rc<Cell<(i32, i32)>>,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Fingerprint & Lock", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let fingerprints = Rc::new(Fingerprints {
        group: ui::group(),
        status: status.clone(),
        device: RefCell::default(),
        user: glib::user_name().to_string_lossy().into_owned(),
        enrolling: RefCell::default(),
        stage: Rc::default(),
    });
    column.append(&ui::section("Fingerprints", None, &fingerprints.group));
    glib::spawn_future_local({
        let fingerprints = fingerprints.clone();
        async move { fingerprints.connect().await }
    });

    let report = {
        let status = status.clone();
        move |result: Result<(), String>| {
            status.set_visible(result.is_err());
            if let Err(error) = result {
                status.set_label(&error);
            }
        }
    };
    let lock = config::read("lock");
    let unlocking = ui::group();
    let unlock = ui::switch(config::get_bool(&lock, "fingerprint").unwrap_or(true), {
        let report = report.clone();
        move |on| report(config::edit("lock", |document| config::set_in(document, "fingerprint", on)))
    });
    unlock.update_property(&[gtk::accessible::Property::Label("Unlock the screen with a fingerprint")]);
    unlocking.append(&ui::row("Unlock the screen with a fingerprint", None, &unlock));
    let sudo = sudo_switch(report.clone());
    unlocking.append(&ui::row("Use a fingerprint for sudo", Some("Your password still works too"), &sudo));
    column.append(&unlocking);

    let locking = ui::group();
    let after = config::get_int(&config::read("power"), "lock_after").unwrap_or(0);
    let choice = ui::choice(&LOCK_AFTER, &after.to_string(), {
        let report = report.clone();
        move |value| {
            let seconds: i64 = value.parse().unwrap_or(0);
            report(config::edit("power", |document| config::set_in(document, "lock_after", seconds)));
        }
    });
    choice.update_property(&[gtk::accessible::Property::Label("Lock after the screen turns off")]);
    locking.append(&ui::row("Lock after the screen turns off", None, &choice));
    let texts = ui::switch(config::get_bool(&lock, "notification_text").unwrap_or(false), {
        let report = report.clone();
        move |on| report(config::edit("lock", |document| config::set_in(document, "notification_text", on)))
    });
    texts.update_property(&[gtk::accessible::Property::Label("Show notification text on the lock screen")]);
    locking.append(&ui::row("Show notification text on the lock screen", None, &texts));
    let password = ui::button("Change password…", |button| super::password::open(button.root().and_downcast::<gtk::Window>().as_ref()));
    locking.append(&ui::row("Password", None, &password));
    column.append(&locking);
    column.append(&status);
    page.upcast()
}

impl Fingerprints {
    async fn connect(self: &Rc<Self>) {
        let connected = async {
            let connection = bus::system().await?;
            let path = ManagerProxy::new(&connection).await?.get_default_device().await?;
            DeviceProxy::builder(&connection).path(path).build().await
        };
        match connected.await {
            Ok(device) => {
                *self.device.borrow_mut() = Some(device);
                self.refresh().await;
            }
            Err(zbus::Error::MethodError(name, ..)) if name.as_str() == "net.reactivated.Fprint.Error.NoSuchDevice" => self.unavailable("There’s no fingerprint sensor."),
            Err(error) => self.unavailable(&bus::message(error, FPRINTD)),
        }
    }

    fn unavailable(&self, text: &str) {
        ui::clear(&self.group);
        let label = ui::note(text);
        label.add_css_class("unavailable");
        label.set_wrap(true);
        self.group.append(&label);
    }

    /// Lists the enrolled fingers, then the add row.
    async fn refresh(self: &Rc<Self>) {
        let Some(device) = self.device.borrow().clone() else { return };
        let enrolled = match device.list_enrolled_fingers(&self.user).await {
            Ok(fingers) => fingers,
            Err(zbus::Error::MethodError(name, ..)) if name.as_str() == "net.reactivated.Fprint.Error.NoEnrolledPrints" => Vec::new(),
            Err(error) => {
                self.unavailable(&bus::message(error, FPRINTD));
                return;
            }
        };
        ui::clear(&self.group);
        for finger in &enrolled {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            line.append(&glyph(20));
            let name = label(finger);
            let text = gtk::Label::new(Some(&name));
            text.add_css_class("row-label");
            text.set_xalign(0.0);
            text.set_hexpand(true);
            line.append(&text);
            let fingerprints = self.clone();
            let finger = finger.clone();
            let delete = ui::button("Delete", move |_| {
                let (fingerprints, finger) = (fingerprints.clone(), finger.clone());
                glib::spawn_future_local(async move { fingerprints.delete(&finger).await });
            });
            delete.add_css_class("danger");
            delete.update_property(&[gtk::accessible::Property::Label(&format!("Delete {name}"))]);
            line.append(&delete);
            self.group.append(&line);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        line.add_css_class("row");
        let next = FINGERS.iter().find(|(id, _)| !enrolled.iter().any(|finger| finger == id)).map(|(id, _)| *id);
        let fingerprints = self.clone();
        let add = ui::button("Add a fingerprint…", move |_| {
            if let Some(finger) = next {
                let fingerprints = fingerprints.clone();
                glib::spawn_future_local(async move { fingerprints.enroll(finger).await });
            }
        });
        add.set_sensitive(next.is_some() && self.enrolling.borrow().is_none());
        line.append(&add);
        line.append(&ui::note("Up to 10, read by fprintd"));
        self.group.append(&line);
    }

    fn report(&self, error: Option<String>) {
        self.status.set_visible(error.is_some());
        if let Some(error) = error {
            self.status.set_label(&error);
        }
    }

    async fn delete(self: &Rc<Self>, finger: &str) {
        let Some(device) = self.device.borrow().clone() else { return };
        let deleted = async {
            device.claim(&self.user).await?;
            let result = device.delete_enrolled_finger(finger).await;
            let _ = device.release().await;
            result
        };
        self.report(deleted.await.err().map(|error| bus::message(error, FPRINTD)));
        self.refresh().await;
    }

    /// Enrols `finger`, showing each stage, until it's done, fails or is cancelled.
    async fn enroll(self: &Rc<Self>, finger: &'static str) {
        let Some(device) = self.device.borrow().clone() else { return };
        if let Err(error) = device.claim(&self.user).await {
            self.report(Some(bus::message(error, FPRINTD)));
            return;
        }
        let stages = device.num_enroll_stages().await.unwrap_or(-1);
        self.stage.set((0, stages));
        let mut statuses = match device.receive_enroll_status().await {
            Ok(statuses) => statuses,
            Err(error) => {
                let _ = device.release().await;
                self.report(Some(bus::message(error, FPRINTD)));
                return;
            }
        };
        if let Err(error) = device.enroll_start(finger).await {
            let _ = device.release().await;
            self.report(Some(bus::message(error, FPRINTD)));
            return;
        }
        self.report(None);
        let (cancel, cancelled) = async_channel::bounded(1);
        self.show_enrolling(cancel);
        let mut problem = None;
        loop {
            let status = match futures_util::future::select(statuses.next(), std::pin::pin!(cancelled.recv())).await {
                futures_util::future::Either::Left((Some(status), _)) => status,
                _ => break,
            };
            let Ok(args) = status.args() else { continue };
            let (result, done) = (args.result.to_owned(), args.done);
            let (step, total) = self.stage.get();
            let hint = match result.as_str() {
                "enroll-stage-passed" => {
                    self.stage.set((step + 1, total));
                    None
                }
                "enroll-completed" => None,
                "enroll-retry-scan" => Some("Didn’t catch that. Touch the sensor again"),
                "enroll-swipe-too-short" => Some("Too short. Touch the sensor again"),
                "enroll-finger-not-centered" => Some("Put your finger in the middle of the sensor"),
                "enroll-remove-and-retry" => Some("Lift your finger and touch again"),
                "enroll-duplicate" => Some("That finger is already enrolled"),
                "enroll-data-full" => Some("The sensor has no room for another fingerprint"),
                "enroll-disconnected" => Some("The sensor was disconnected"),
                _ => Some("The sensor couldn’t enrol that finger"),
            };
            self.update_enrolling(hint);
            if done {
                if result != "enroll-completed" {
                    problem = hint.map(str::to_owned);
                }
                break;
            }
        }
        let _ = device.enroll_stop().await;
        let _ = device.release().await;
        self.enrolling.borrow_mut().take();
        self.report(problem);
        self.refresh().await;
    }

    /// The row the design shows while enrolling: a ring filling by stage, what to do next, and Cancel.
    fn show_enrolling(self: &Rc<Self>, cancel: async_channel::Sender<()>) {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        line.add_css_class("row");
        line.add_css_class("enrolling");
        let ring = gtk::DrawingArea::builder().content_width(52).content_height(52).valign(gtk::Align::Center).build();
        let stage = self.stage.clone();
        ring.set_draw_func(move |_, cr, width, height| {
            let look = theme::look();
            let (done, total) = stage.get();
            let (x, y, radius) = (f64::from(width) / 2.0, f64::from(height) / 2.0, f64::from(width.min(height)) / 2.0 - 2.0);
            theme::set_source(cr, &look.palette.text, 0.10);
            cr.set_line_width(4.0);
            cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
            cr.stroke().ok();
            if total > 0 {
                theme::set_source(cr, &look.palette.accent, 1.0);
                let start = -std::f64::consts::FRAC_PI_2;
                cr.arc(x, y, radius, start, start + std::f64::consts::TAU * f64::from(done.min(total)) / f64::from(total));
                cr.stroke().ok();
            }
            cr.translate(x - 10.0, y - 10.0);
            theme::set_source(cr, &look.palette.accent, 1.0);
            icon::stroke(cr, GLYPH, 20.0);
        });
        line.append(&ring);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some("Touch the sensor"));
        title.add_css_class("row-label");
        title.set_xalign(0.0);
        let note = ui::note("");
        text.append(&title);
        text.append(&note);
        line.append(&text);
        let cancel = ui::button("Cancel", move |_| {
            let _ = cancel.try_send(());
        });
        line.append(&cancel);
        // Above the add row.
        let add = self.group.last_child();
        self.group.insert_child_after(&line, add.as_ref().and_then(|add| add.prev_sibling()).as_ref());
        if let Some(add) = add.and_then(|add| add.first_child()) {
            add.set_sensitive(false);
        }
        *self.enrolling.borrow_mut() = Some((text, note, ring));
        self.update_enrolling(None);
    }

    fn update_enrolling(&self, hint: Option<&str>) {
        let enrolling = self.enrolling.borrow();
        let Some((text, note, ring)) = enrolling.as_ref() else { return };
        let (done, total) = self.stage.get();
        if let Some(title) = text.first_child().and_downcast::<gtk::Label>() {
            title.set_label(hint.unwrap_or(if done == 0 { "Touch the sensor" } else { "Touch the sensor again" }));
        }
        let count = if total > 0 { format!(" · {done} of {total}") } else { String::new() };
        note.set_label(&format!("Lift and rest your finger, a little differently each time{count}"));
        ring.queue_draw();
    }
}

fn glyph(size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(size).content_height(size).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, _, _| {
        theme::set_source(cr, &theme::look().palette.accent, 1.0);
        icon::stroke(cr, GLYPH, f64::from(size));
    });
    theme::redraw_on_change(&area);
    area
}

/// "right-index-finger" as "Right index finger".
fn label(finger: &str) -> String {
    FINGERS.iter().find(|(id, _)| *id == finger).map(|(_, name)| name.to_string()).unwrap_or_else(|| finger.replace('-', " "))
}

/// Whether `/etc/pam.d/autark-fingerprint` lets a fingerprint through.
fn sudo_on() -> bool {
    std::fs::read_to_string(SUDO_FILE).is_ok_and(|text| text.lines().any(|line| !line.trim_start().starts_with('#') && line.contains("pam_fprintd.so")))
}

/// The sudo switch: flipping it asks for the password, then runs the helper through sudo.
fn sudo_switch(report: impl Fn(Result<(), String>) + Clone + 'static) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(sudo_on());
    switch.set_valign(gtk::Align::Center);
    switch.update_property(&[gtk::accessible::Property::Label("Use a fingerprint for sudo")]);
    if !std::path::Path::new(HELPER).exists() {
        switch.set_sensitive(false);
        switch.set_tooltip_text(Some("autark-fingerprint-sudo isn’t installed"));
    }
    let asking = Rc::new(Cell::new(false));
    switch.connect_state_set(move |switch, on| {
        if asking.get() {
            return glib::Propagation::Stop;
        }
        asking.set(true);
        let (switch, asking, report) = (switch.clone(), asking.clone(), report.clone());
        ask_password(switch.root().and_downcast::<gtk::Window>().as_ref(), move |password| {
            let (switch, asking, report) = (switch.clone(), asking.clone(), report.clone());
            glib::spawn_future_local(async move {
                let result = match password {
                    Some(password) => run_helper(on, password).await,
                    None => Ok(()),
                };
                report(result);
                // The switch shows what the file says now, whatever happened.
                switch.set_state(sudo_on());
                switch.set_active(sudo_on());
                asking.set(false);
            });
        });
        glib::Propagation::Stop
    });
    switch
}

async fn run_helper(on: bool, password: Zeroizing<String>) -> Result<(), String> {
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        use std::io::Write;
        let result = (|| {
            let mut child = std::process::Command::new("sudo")
                .args(["-S", "-k", "-p", "", HELPER, if on { "on" } else { "off" }])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|error| format!("sudo: {error}"))?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(format!("{}\n", password.as_str()).as_bytes());
            }
            let output = child.wait_with_output().map_err(|error| format!("sudo: {error}"))?;
            if output.status.success() {
                return Ok(());
            }
            let said = String::from_utf8_lossy(&output.stderr).lines().rfind(|line| !line.trim().is_empty()).unwrap_or("").to_owned();
            Err(if said.contains("incorrect password") || said.contains("try again") { "That password isn’t right.".to_owned() } else { format!("sudo: {said}") })
        })();
        let _ = sender.send_blocking(result);
    });
    receiver.recv().await.unwrap_or_else(|_| Err("sudo stopped unexpectedly.".into()))
}

/// A small dialog for the password sudo needs; `answered` gets it, or nothing when cancelled.
fn ask_password(parent: Option<&gtk::Window>, answered: impl Fn(Option<Zeroizing<String>>) + 'static) {
    let window = gtk::Window::builder().title("Password").modal(true).resizable(false).default_width(340).build();
    window.set_transient_for(parent);
    // Settings' own app id, so the bar and dock count it as Settings.
    window.set_application(parent.and_then(|parent| parent.application()).as_ref());
    window.add_css_class("settings");
    window.add_css_class("dialog");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.add_css_class("page");
    let note = ui::note("Settings needs your password to change how sudo checks who you are.");
    note.set_wrap(true);
    column.append(&note);
    let entry = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text("Password").build();
    entry.add_css_class("field");
    entry.update_property(&[gtk::accessible::Property::Label("Password")]);
    column.append(&entry);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let answered = Rc::new(RefCell::new(Some(Box::new(answered) as Box<dyn Fn(Option<Zeroizing<String>>)>)));
    let finish = {
        let (answered, window) = (answered.clone(), window.downgrade());
        move |password: Option<Zeroizing<String>>| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingers_are_named() {
        assert_eq!(label("left-thumb"), "Left thumb");
        assert_eq!(label("some-new-finger"), "some new finger");
    }
}
