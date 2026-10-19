//! Bluetooth, through BlueZ: turning it on and off, the paired devices (connect, disconnect, forget, with
//! their battery when they report it), and the ones nearby to pair, with the passkey to confirm shown in
//! the row. While the pane is on screen the laptop searches and can be found under its name.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use autark_common::theme;
use gtk::glib;
use gtk::prelude::*;

use crate::bluez::{self, Ask, Snapshot};
use crate::{bus, icon, ui};

/// A pairing under way, started here.
struct Pairing {
    path: String,
    /// What to tell the person: the passkey to compare, the code to type, or that it's waiting.
    note: String,
    /// Waiting for "Confirm".
    answer: Option<async_channel::Sender<bool>>,
}

struct Bluetooth {
    connection: RefCell<Option<zbus::Connection>>,
    snapshot: RefCell<Snapshot>,
    agent: Cell<bool>,
    top: gtk::Box,
    lists: gtk::Box,
    status: gtk::Label,
    pairing: RefCell<Option<Pairing>>,
    /// Devices connecting or disconnecting, so their button waits.
    busy: RefCell<Vec<String>>,
    later: ui::Debounce,
    shown: Cell<bool>,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Bluetooth", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let bluetooth = Rc::new(Bluetooth {
        connection: RefCell::default(),
        snapshot: RefCell::default(),
        agent: Cell::new(false),
        top: ui::group(),
        lists: gtk::Box::new(gtk::Orientation::Vertical, 18),
        status: status.clone(),
        pairing: RefCell::default(),
        busy: RefCell::default(),
        later: ui::Debounce::default(),
        shown: Cell::new(false),
    });
    column.append(&bluetooth.top);
    column.append(&bluetooth.lists);
    column.append(&status);
    page.connect_map({
        let bluetooth = bluetooth.clone();
        move |_| {
            bluetooth.shown.set(true);
            bluetooth.visible(true);
        }
    });
    page.connect_unmap({
        let bluetooth = bluetooth.clone();
        move |_| {
            bluetooth.shown.set(false);
            bluetooth.visible(false);
        }
    });
    glib::spawn_future_local({
        let bluetooth = bluetooth.clone();
        async move { bluetooth.start().await }
    });
    page.upcast()
}

impl Bluetooth {
    async fn start(self: &Rc<Self>) {
        let connection = match bus::system().await {
            Ok(connection) => connection,
            Err(error) => return self.unavailable(&bus::message(error, bluez::DAEMON)),
        };
        *self.connection.borrow_mut() = Some(connection.clone());
        self.update().await;
        let weak = Rc::downgrade(self);
        let watched = bluez::watch(&connection, move || {
            if let Some(bluetooth) = weak.upgrade() {
                let again = bluetooth.clone();
                bluetooth.later.run(Duration::from_millis(150), move || {
                    glib::spawn_future_local(async move { again.update().await });
                });
            }
        })
        .await;
        if let Err(error) = watched {
            eprintln!("settings: watching BlueZ: {error}");
        }
    }

    fn connection(&self) -> Option<zbus::Connection> {
        self.connection.borrow().clone()
    }

    /// Reads BlueZ afresh, registering the pairing agent when BlueZ is new, or says it isn't running.
    async fn update(self: &Rc<Self>) {
        let Some(connection) = self.connection() else { return };
        match bluez::read(&connection).await {
            Ok(snapshot) => {
                let was_off = !self.snapshot.borrow().adapter.as_ref().is_some_and(|adapter| adapter.powered);
                *self.snapshot.borrow_mut() = snapshot;
                self.render();
                if !self.agent.get() {
                    self.listen(&connection).await;
                }
                // Turned on (or BlueZ started) while shown: search and be found now.
                if was_off && self.shown.get() {
                    self.visible(true);
                }
            }
            Err(error) => {
                self.agent.set(false);
                *self.snapshot.borrow_mut() = Snapshot::default();
                self.unavailable(&bus::message(error, bluez::DAEMON));
            }
        }
    }

    /// Takes the agent's questions as they come.
    async fn listen(self: &Rc<Self>, connection: &zbus::Connection) {
        let asks = match bluez::agent(connection).await {
            Ok(asks) => asks,
            Err(error) => return eprintln!("settings: BlueZ agent: {error}"),
        };
        self.agent.set(true);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(ask) = asks.recv().await {
                let Some(bluetooth) = weak.upgrade() else { return };
                bluetooth.asked(ask);
            }
        });
    }

    fn asked(self: &Rc<Self>, ask: Ask) {
        let mut pairing = self.pairing.borrow_mut();
        match ask {
            Ask::Confirm { device, passkey, answer } => match pairing.as_mut().filter(|pairing| pairing.path == device) {
                Some(pairing) => {
                    pairing.note = format!("Pairing… confirm {} on the device", bluez::spaced(&format!("{passkey:06}")));
                    pairing.answer = Some(answer);
                }
                // Not started here: refused.
                None => {
                    let _ = answer.try_send(false);
                }
            },
            Ask::Show { device, code } => {
                if let Some(pairing) = pairing.as_mut().filter(|pairing| pairing.path == device) {
                    pairing.note = format!("Type {} on the device, then press Enter", bluez::spaced(&code));
                }
            }
            Ask::Done => {
                if let Some(pairing) = pairing.as_mut() {
                    pairing.answer = None;
                }
            }
        }
        drop(pairing);
        self.render();
    }

    /// While shown: searching for devices, and findable by them.
    fn visible(self: &Rc<Self>, on: bool) {
        let Some(connection) = self.connection() else { return };
        let Some(adapter) = self.snapshot.borrow().adapter.clone().filter(|adapter| adapter.powered) else { return };
        glib::spawn_future_local(async move {
            let Ok(proxy) = bluez::AdapterProxy::builder(&connection).path(adapter.path).build().await else { return };
            // Already searching, or not, is as good.
            if on {
                let _ = proxy.start_discovery().await;
            } else {
                let _ = proxy.stop_discovery().await;
            }
            let _ = proxy.set_discoverable(on).await;
        });
    }

    fn unavailable(self: &Rc<Self>, text: &str) {
        ui::clear(&self.top);
        ui::clear(&self.lists);
        let label = ui::note(&format!("{text} Bluetooth is set through BlueZ."));
        label.add_css_class("unavailable");
        label.set_wrap(true);
        self.top.append(&label);
        // The service is usually just off, which is root's to start; the helper does it with your password.
        let turn_on = ui::button("Turn on Bluetooth", {
            let this = self.clone();
            move |button| {
                let again = this.clone();
                crate::privileged::apply(button, "turn Bluetooth on", &["service", "enable", "bluetooth"], None, &this.status, move |_| {
                    glib::spawn_future_local(async move {
                        glib::timeout_future_seconds(2).await;
                        again.start().await;
                    });
                });
            }
        });
        turn_on.set_halign(gtk::Align::Start);
        self.top.append(&turn_on);
    }

    fn report(&self, error: Option<String>) {
        self.status.set_visible(error.is_some());
        if let Some(error) = error {
            self.status.set_label(&error);
        }
    }

    fn render(self: &Rc<Self>) {
        ui::rebuild_keeping_focus(&[&self.top, &self.lists], || self.build_page());
    }

    fn build_page(self: &Rc<Self>) {
        let snapshot = self.snapshot.borrow().clone();
        ui::clear(&self.top);
        ui::clear(&self.lists);
        let Some(adapter) = snapshot.adapter.clone() else {
            let label = ui::note("There’s no Bluetooth adapter.");
            label.add_css_class("unavailable");
            self.top.append(&label);
            return;
        };
        let bluetooth = self.clone();
        let path = adapter.path.clone();
        let power = ui::switch(adapter.powered, move |on| {
            let (bluetooth, path) = (bluetooth.clone(), path.clone());
            glib::spawn_future_local(async move {
                let Some(connection) = bluetooth.connection() else { return };
                let result = async { bluez::AdapterProxy::builder(&connection).path(path).build().await?.set_powered(on).await };
                bluetooth.report(result.await.err().map(|error| bus::message(error, bluez::DAEMON)));
            });
        });
        power.update_property(&[gtk::accessible::Property::Label("Bluetooth")]);
        power.set_widget_name("bt:power");
        let name = if adapter.alias.is_empty() { "this laptop".to_owned() } else { format!("“{}”", adapter.alias) };
        let note = adapter.powered.then(|| format!("This laptop shows up as {name} while this pane is open"));
        self.top.append(&ui::row("Bluetooth", note.as_deref(), &power));
        if !adapter.powered {
            return;
        }

        let group = ui::group();
        for device in &snapshot.mine {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            line.append(&tile(&device.icon));
            let status = match (device.connected, device.battery) {
                (true, Some(battery)) => format!("Connected · battery {battery}%"),
                (true, None) => "Connected".to_owned(),
                (false, _) => "Not connected".to_owned(),
            };
            let texts = texts(&device.name, &status);
            if device.connected {
                texts.last_child().inspect(|note| note.add_css_class("good"));
            }
            line.append(&texts);
            let busy = self.busy.borrow().contains(&device.path.to_string());
            let bluetooth = self.clone();
            let (path, connected) = (device.path.to_string(), device.connected);
            let action = ui::button(
                match (busy, connected) {
                    (true, true) => "Disconnecting…",
                    (true, false) => "Connecting…",
                    (false, true) => "Disconnect",
                    (false, false) => "Connect",
                },
                move |_| bluetooth.connect(path.clone(), !connected),
            );
            action.set_sensitive(!busy);
            action.set_widget_name(&format!("bt:connect:{}", device.path));
            line.append(&action);
            let bluetooth = self.clone();
            let (path, adapter_path) = (device.path.clone(), adapter.path.clone());
            let forget = ui::button("Forget", move |_| {
                let (bluetooth, path, adapter_path) = (bluetooth.clone(), path.clone(), adapter_path.clone());
                glib::spawn_future_local(async move {
                    let Some(connection) = bluetooth.connection() else { return };
                    let result = async { bluez::AdapterProxy::builder(&connection).path(adapter_path).build().await?.remove_device(&path).await };
                    bluetooth.report(result.await.err().map(|error| bus::message(error, bluez::DAEMON)));
                });
            });
            forget.add_css_class("danger");
            forget.set_widget_name(&format!("bt:forget:{}", device.path));
            forget.update_property(&[gtk::accessible::Property::Label(&format!("Forget {}", device.name))]);
            line.append(&forget);
            group.append(&line);
        }
        if snapshot.mine.is_empty() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            line.append(&ui::note("No devices paired yet"));
            group.append(&line);
        }
        self.lists.append(&ui::section("My devices", None, &group));

        let group = ui::group();
        let pairing = self.pairing.borrow();
        for device in &snapshot.nearby {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            line.append(&tile(&device.icon));
            let this = pairing.as_ref().filter(|pairing| pairing.path == device.path.as_str());
            line.append(&texts(&device.name, this.map_or("Ready to pair", |pairing| pairing.note.as_str())));
            let bluetooth = self.clone();
            let path = device.path.to_string();
            let (label, waiting) = match this {
                Some(pairing) if pairing.answer.is_some() => ("Confirm", false),
                Some(_) => ("Pair", true),
                None => ("Pair", pairing.is_some()),
            };
            let name = device.name.clone();
            let action = ui::button(label, move |_| bluetooth.pair_or_confirm(path.clone(), name.clone()));
            action.set_sensitive(!waiting);
            action.set_widget_name(&format!("bt:pair:{}", device.path));
            action.update_property(&[gtk::accessible::Property::Label(&format!("{label} {}", device.name))]);
            if this.is_some() {
                let bluetooth = self.clone();
                let path = device.path.to_string();
                let cancel = ui::button("Cancel", move |_| bluetooth.cancel_pairing(path.clone()));
                cancel.set_widget_name(&format!("bt:cancel:{}", device.path));
                line.append(&cancel);
            }
            line.append(&action);
            group.append(&line);
        }
        if snapshot.nearby.is_empty() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            line.append(&ui::note("Nothing else nearby. Put a device in pairing mode to see it here."));
            group.append(&line);
        }
        let aside = ui::note(if adapter.discovering { "Searching…" } else { "" });
        self.lists.append(&ui::section("Nearby", Some(&aside), &group));
    }

    fn connect(self: &Rc<Self>, path: String, on: bool) {
        let Some(connection) = self.connection() else { return };
        self.busy.borrow_mut().push(path.clone());
        self.render();
        let bluetooth = self.clone();
        glib::spawn_future_local(async move {
            let result = async {
                let device = bluez::DeviceProxy::builder(&connection).path(path.as_str()).build().await?;
                if on { device.connect().await } else { device.disconnect().await }
            };
            let result = result.await;
            bluetooth.busy.borrow_mut().retain(|busy| *busy != path);
            bluetooth.report(result.err().map(|error| failed(if on { "Couldn’t connect" } else { "Couldn’t disconnect" }, error)));
            bluetooth.render();
        });
    }

    /// Pair, then trust and connect; or, while a passkey waits, say it matches.
    fn pair_or_confirm(self: &Rc<Self>, path: String, name: String) {
        let answer = self.pairing.borrow_mut().as_mut().filter(|pairing| pairing.path == path).and_then(|pairing| {
            pairing.note = "Pairing…".into();
            pairing.answer.take()
        });
        if let Some(answer) = answer {
            let _ = answer.try_send(true);
            self.render();
            return;
        }
        let Some(connection) = self.connection() else { return };
        *self.pairing.borrow_mut() = Some(Pairing { path: path.clone(), note: "Pairing…".into(), answer: None });
        self.report(None);
        self.render();
        let bluetooth = self.clone();
        glib::spawn_future_local(async move {
            let result = async {
                let device = bluez::DeviceProxy::builder(&connection).path(path.as_str()).build().await?;
                device.pair().await?;
                device.set_trusted(true).await?;
                // Paired is what was asked; a device that won't connect at once can be connected later.
                let _ = device.connect().await;
                Ok::<(), zbus::Error>(())
            };
            let result = result.await;
            bluetooth.pairing.borrow_mut().take();
            bluetooth.report(result.err().map(|error| failed(&format!("Couldn’t pair with {name}"), error)));
            bluetooth.render();
        });
    }

    fn cancel_pairing(self: &Rc<Self>, path: String) {
        let answer = self.pairing.borrow_mut().as_mut().filter(|pairing| pairing.path == path).and_then(|pairing| pairing.answer.take());
        if let Some(answer) = answer {
            let _ = answer.try_send(false);
        }
        let Some(connection) = self.connection() else { return };
        glib::spawn_future_local(async move {
            if let Ok(device) = bluez::DeviceProxy::builder(&connection).path(path.as_str()).build().await {
                let _ = device.cancel_pairing().await;
            }
        });
    }
}

/// BlueZ's error names, in words.
fn failed(what: &str, error: zbus::Error) -> String {
    let reason = match &error {
        zbus::Error::MethodError(name, ..) => match name.as_str() {
            "org.bluez.Error.AuthenticationCanceled" | "org.bluez.Error.AuthenticationRejected" => Some("it was cancelled"),
            "org.bluez.Error.AuthenticationFailed" | "org.bluez.Error.AuthenticationTimeout" => Some("the device didn’t confirm. Put it in pairing mode and try again"),
            "org.bluez.Error.ConnectionAttemptFailed" | "org.bluez.Error.Failed" => Some("the device didn’t answer. Is it on and close by?"),
            "org.bluez.Error.InProgress" => Some("it’s already under way"),
            "org.bluez.Error.Blocked" | "org.bluez.Error.NotReady" => Some("Bluetooth is off or blocked"),
            _ => None,
        },
        _ => None,
    };
    match reason {
        Some(reason) => format!("{what}: {reason}."),
        None => format!("{what}: {}", bus::message(error, bluez::DAEMON)),
    }
}

/// A device's glyph on a quiet tile.
fn tile(icon_name: &str) -> gtk::DrawingArea {
    let glyph = bluez::glyph(icon_name);
    let area = gtk::DrawingArea::builder().content_width(32).content_height(32).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, width, height| {
        let look = theme::look();
        theme::set_source(cr, &look.palette.text, 0.07);
        let (width, height, radius) = (f64::from(width), f64::from(height), 8.0);
        cr.new_sub_path();
        cr.arc(width - radius, radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(width - radius, height - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(radius, height - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(radius, radius, radius, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.close_path();
        cr.fill().ok();
        theme::set_source(cr, &look.palette.text, 1.0);
        cr.translate(7.0, 7.0);
        icon::stroke(cr, glyph, 18.0);
    });
    theme::redraw_on_change(&area);
    area
}

fn texts(title: &str, note: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_hexpand(true);
    column.set_valign(gtk::Align::Center);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("row-label");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    column.append(&label);
    let note = ui::note(note);
    note.set_xalign(0.0);
    note.set_wrap(true);
    column.append(&note);
    column
}
