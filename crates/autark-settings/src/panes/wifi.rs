//! Wi-Fi, through iwd: turning it on and off, the network it's on (address and security) with Disconnect,
//! the known networks (joining automatically or not, forgetting them), the others in range with a password
//! asked in the row, and joining a hidden network. The page follows iwd's signals and rebuilds from a
//! fresh read each time; it scans while it's on screen.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use autark_common::theme;
use gtk::glib;
use gtk::prelude::*;
use zeroize::Zeroizing;

use crate::iwd::{self, Snapshot};
use crate::{bus, icon, ui};

const ARCS: [&str; 3] = ["M8.5 15.5a5 5 0 0 1 7 0", "M5 12a10 10 0 0 1 14 0", "M2 8.5a15 15 0 0 1 20 0"];
const DOT: &str = "M12 19h.01";
const LOCK: &str = "M6 11h12v10H6zM8.5 11V8a3.5 3.5 0 0 1 7 0v3";
/// How often it scans while the page is shown and connected (iwd scans by itself while disconnected).
const SCAN_EVERY: Duration = Duration::from_secs(30);

struct Wifi {
    connection: RefCell<Option<zbus::Connection>>,
    passphrase: RefCell<Option<iwd::Passphrase>>,
    snapshot: RefCell<Snapshot>,
    /// The security of the network it's on, from iwd's diagnostics.
    security: RefCell<Option<String>>,
    top: gtk::Box,
    lists: gtk::Box,
    status: gtk::Label,
    /// The network asking for its password, and the field it's typed in, kept across rebuilds.
    asking: RefCell<Option<(String, gtk::PasswordEntry)>>,
    /// A join in progress, by network path.
    joining: RefCell<Option<String>>,
    later: ui::Debounce,
    shown: Cell<bool>,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Wi-Fi", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let wifi = Rc::new(Wifi {
        connection: RefCell::default(),
        passphrase: RefCell::default(),
        snapshot: RefCell::default(),
        security: RefCell::default(),
        top: ui::group(),
        lists: gtk::Box::new(gtk::Orientation::Vertical, 18),
        status: status.clone(),
        asking: RefCell::default(),
        joining: RefCell::default(),
        later: ui::Debounce::default(),
        shown: Cell::new(false),
    });
    column.append(&wifi.top);
    column.append(&wifi.lists);
    column.append(&status);

    page.connect_map({
        let wifi = wifi.clone();
        move |_| {
            wifi.shown.set(true);
            wifi.scan();
        }
    });
    page.connect_unmap({
        let wifi = wifi.clone();
        move |_| wifi.shown.set(false)
    });
    glib::timeout_add_local(SCAN_EVERY, {
        let wifi = Rc::downgrade(&wifi);
        move || match wifi.upgrade() {
            Some(wifi) => {
                if wifi.shown.get() {
                    wifi.scan();
                }
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        }
    });
    glib::spawn_future_local({
        let wifi = wifi.clone();
        async move { wifi.start().await }
    });
    page.upcast()
}

impl Wifi {
    async fn start(self: &Rc<Self>) {
        let connection = match bus::system().await {
            Ok(connection) => connection,
            Err(error) => return self.unavailable(&bus::message(error, iwd::DAEMON)),
        };
        *self.connection.borrow_mut() = Some(connection.clone());
        self.update().await;
        let weak = Rc::downgrade(self);
        let watched = iwd::watch(&connection, move || {
            if let Some(wifi) = weak.upgrade() {
                let again = wifi.clone();
                wifi.later.run(Duration::from_millis(150), move || {
                    glib::spawn_future_local(async move { again.update().await });
                });
            }
        })
        .await;
        if let Err(error) = watched {
            eprintln!("settings: watching iwd: {error}");
        }
    }

    /// Reads iwd afresh, registering the passphrase agent when iwd is new, or says it isn't running.
    async fn update(self: &Rc<Self>) {
        let Some(connection) = self.connection() else { return };
        match self.refresh().await {
            Ok(()) => {
                if self.passphrase.borrow().is_none() {
                    match iwd::agent(&connection).await {
                        Ok(passphrase) => *self.passphrase.borrow_mut() = Some(passphrase),
                        Err(error) => eprintln!("settings: iwd agent: {error}"),
                    }
                }
            }
            Err(error) => {
                // A restarted iwd knows no agent.
                self.passphrase.borrow_mut().take();
                *self.snapshot.borrow_mut() = Snapshot::default();
                self.unavailable(&bus::message(error, iwd::DAEMON));
            }
        }
    }

    fn connection(&self) -> Option<zbus::Connection> {
        self.connection.borrow().clone()
    }

    fn unavailable(&self, text: &str) {
        ui::clear(&self.top);
        ui::clear(&self.lists);
        let label = ui::note(&format!("{text} Wi-Fi is set through iwd."));
        label.add_css_class("unavailable");
        label.set_wrap(true);
        self.top.append(&label);
    }

    fn report(&self, error: Option<String>) {
        self.status.set_visible(error.is_some());
        if let Some(error) = error {
            self.status.set_label(&error);
        }
    }

    fn scan(self: &Rc<Self>) {
        let Some(connection) = self.connection() else { return };
        let Some(device) = self.snapshot.borrow().device.clone().filter(|device| device.state.is_some()) else { return };
        glib::spawn_future_local(async move {
            if let Ok(station) = iwd::StationProxy::builder(&connection).path(device.path).build().await {
                // Refused while iwd is already scanning, which is as good.
                let _ = station.scan().await;
            }
        });
    }

    async fn refresh(self: &Rc<Self>) -> zbus::Result<()> {
        let Some(connection) = self.connection() else { return Ok(()) };
        let snapshot = iwd::read(&connection).await?;
        let security = match &snapshot.device {
            Some(device) if device.connected.is_some() => iwd::security(&connection, &device.path).await,
            _ => None,
        };
        let first = self.snapshot.borrow().device.is_none();
        *self.snapshot.borrow_mut() = snapshot;
        *self.security.borrow_mut() = security;
        self.render();
        if first && self.shown.get() {
            self.scan();
        }
        Ok(())
    }

    /// Rebuilds the page from the last read, keeping keyboard focus where it was.
    fn render(self: &Rc<Self>) {
        ui::rebuild_keeping_focus(&[&self.top, &self.lists], || self.build_page());
    }

    fn build_page(self: &Rc<Self>) {
        let snapshot = self.snapshot.borrow().clone();
        let security = self.security.borrow().clone();
        ui::clear(&self.top);
        ui::clear(&self.lists);
        let Some(device) = snapshot.device.clone() else {
            let label = ui::note("There’s no Wi-Fi device.");
            label.add_css_class("unavailable");
            self.top.append(&label);
            return;
        };
        let wifi = self.clone();
        let path = device.path.clone();
        let power = ui::switch(device.powered, move |on| {
            let (wifi, path) = (wifi.clone(), path.clone());
            glib::spawn_future_local(async move {
                let Some(connection) = wifi.connection() else { return };
                let result = async { iwd::DeviceProxy::builder(&connection).path(path).build().await?.set_powered(on).await };
                wifi.report(result.await.err().map(|error| bus::message(error, iwd::DAEMON)));
            });
        });
        power.update_property(&[gtk::accessible::Property::Label("Wi-Fi")]);
        power.set_widget_name("wifi:power");
        self.top.append(&ui::row("Wi-Fi", None, &power));
        if !device.powered {
            return;
        }

        let current = snapshot.networks.iter().find(|network| network.connected).cloned();
        match (&current, device.state.as_deref()) {
            (Some(network), _) => {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
                line.add_css_class("row");
                line.append(&tile());
                let mut details = vec!["Connected".to_owned()];
                details.extend(iwd::address(&device.interface));
                details.push(security.unwrap_or_else(|| if network.kind == "open" { "Open".into() } else { "Secured".into() }));
                line.append(&texts(&network.name, &details.join(" · ")));
                let wifi = self.clone();
                let path = device.path.clone();
                let disconnect = ui::button("Disconnect", move |_| {
                    let (wifi, path) = (wifi.clone(), path.clone());
                    glib::spawn_future_local(async move {
                        let Some(connection) = wifi.connection() else { return };
                        let result = async { iwd::StationProxy::builder(&connection).path(path).build().await?.disconnect().await };
                        wifi.report(result.await.err().map(|error| bus::message(error, iwd::DAEMON)));
                    });
                });
                disconnect.set_widget_name("wifi:disconnect");
                line.append(&disconnect);
                self.top.append(&line);
            }
            (None, Some("connecting")) => self.top.append(&ui::row("Connecting…", None, &gtk::Spinner::builder().spinning(true).build())),
            _ => {}
        }

        if !snapshot.known.is_empty() {
            let group = ui::group();
            for known in &snapshot.known {
                let in_range = snapshot.networks.iter().find(|network| network.known.as_ref() == Some(&known.path));
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
                line.add_css_class("row");
                line.append(&fan(in_range.map(|network| iwd::bars(network.signal))));
                line.append(&name(&known.name, known.kind != "open", if in_range.is_none() { Some("Not in range") } else { None }));
                if let Some(network) = in_range.filter(|network| !network.connected) {
                    line.append(&self.join_button(network.path.to_string(), &network.name, false));
                }
                line.append(&ui::note("Join automatically"));
                let wifi = self.clone();
                let path = known.path.clone();
                let auto = ui::switch(known.auto_connect, move |on| {
                    let (wifi, path) = (wifi.clone(), path.clone());
                    glib::spawn_future_local(async move {
                        let Some(connection) = wifi.connection() else { return };
                        let result = async { iwd::KnownNetworkProxy::builder(&connection).path(path).build().await?.set_auto_connect(on).await };
                        wifi.report(result.await.err().map(|error| bus::message(error, iwd::DAEMON)));
                    });
                });
                auto.update_property(&[gtk::accessible::Property::Label(&format!("Join {} automatically", known.name))]);
                auto.set_widget_name(&format!("wifi:auto:{}", known.path));
                line.append(&auto);
                let wifi = self.clone();
                let path = known.path.clone();
                let forget = ui::button("Forget", move |_| {
                    let (wifi, path) = (wifi.clone(), path.clone());
                    glib::spawn_future_local(async move {
                        let Some(connection) = wifi.connection() else { return };
                        let result = async { iwd::KnownNetworkProxy::builder(&connection).path(path).build().await?.forget().await };
                        wifi.report(result.await.err().map(|error| bus::message(error, iwd::DAEMON)));
                    });
                });
                forget.add_css_class("danger");
                forget.update_property(&[gtk::accessible::Property::Label(&format!("Forget {}", known.name))]);
                forget.set_widget_name(&format!("wifi:forget:{}", known.path));
                line.append(&forget);
                group.append(&line);
            }
            self.lists.append(&ui::section("Known networks", Some(&ui::note("Joined automatically when in range")), &group));
        }

        let others: Vec<_> = snapshot.networks.iter().filter(|network| network.known.is_none() && !network.connected).collect();
        let group = ui::group();
        for network in &others {
            let path = network.path.to_string();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            line.append(&fan(Some(iwd::bars(network.signal))));
            let enterprise = network.kind == "8021x";
            line.append(&name(&network.name, network.kind != "open", enterprise.then_some("Needs a provisioning file in /var/lib/iwd")));
            let asking = self.asking.borrow().as_ref().filter(|(asked, _)| *asked == path).map(|(_, entry)| entry.clone());
            if let Some(entry) = asking {
                let had_focus = entry.has_focus();
                if let Some(parent) = entry.parent().and_downcast::<gtk::Box>() {
                    parent.remove(&entry);
                }
                line.append(&entry);
                if had_focus {
                    entry.grab_focus();
                }
                let wifi = self.clone();
                let cancel = ui::button("Cancel", move |_| {
                    wifi.asking.borrow_mut().take();
                    wifi.render();
                });
                cancel.set_widget_name(&format!("wifi:cancel:{path}"));
                line.append(&cancel);
            }
            let join = self.join_button(path, &network.name, network.kind == "psk");
            join.set_sensitive(!enterprise && self.joining.borrow().is_none());
            line.append(&join);
            group.append(&line);
        }
        if others.is_empty() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            line.append(&ui::note(if device.scanning { "Searching…" } else { "No other networks in range" }));
            group.append(&line);
        }
        let aside = ui::note(if device.scanning { "Searching…" } else { "" });
        self.lists.append(&ui::section("Other networks", Some(&aside), &group));

        let wifi = self.clone();
        let hidden = ui::button("Join a hidden network…", move |button| wifi.join_hidden(button.root().and_downcast::<gtk::Window>().as_ref()));
        hidden.set_halign(gtk::Align::Start);
        hidden.set_widget_name("wifi:hidden");
        self.lists.append(&hidden);
    }

    /// Join: a secured network not known yet asks for its password in the row first.
    fn join_button(self: &Rc<Self>, path: String, name: &str, needs_password: bool) -> gtk::Button {
        let wifi = self.clone();
        let label = name.to_owned();
        let widget_name = format!("wifi:join:{path}");
        let button = ui::button(if self.joining.borrow().as_deref() == Some(path.as_str()) { "Joining…" } else { "Join" }, move |_| {
            let typed = wifi.asking.borrow().as_ref().filter(|(asked, _)| *asked == path).map(|(_, entry)| Zeroizing::new(entry.text().to_string()));
            match (needs_password, typed) {
                (true, None) => {
                    let entry = gtk::PasswordEntry::builder().placeholder_text("Password").show_peek_icon(true).width_chars(16).valign(gtk::Align::Center).build();
                    entry.add_css_class("field");
                    entry.update_property(&[gtk::accessible::Property::Label(&format!("Password for {label}"))]);
                    entry.set_widget_name(&format!("wifi:password:{path}"));
                    let (again, asked, label) = (wifi.clone(), path.clone(), label.clone());
                    entry.connect_activate(move |entry| again.join(asked.clone(), label.clone(), Some(Zeroizing::new(entry.text().to_string()))));
                    *wifi.asking.borrow_mut() = Some((path.clone(), entry.clone()));
                    wifi.render();
                    entry.grab_focus();
                }
                (_, typed) => wifi.join(path.clone(), label.clone(), typed),
            }
        });
        button.update_property(&[gtk::accessible::Property::Label(&format!("Join {name}"))]);
        button.set_widget_name(&widget_name);
        button
    }

    fn join(self: &Rc<Self>, path: String, name: String, passphrase: Option<Zeroizing<String>>) {
        let Some(connection) = self.connection() else { return };
        if let Some(slot) = self.passphrase.borrow().as_ref()
            && let Ok(mut slot) = slot.lock()
        {
            *slot = passphrase.clone();
        }
        *self.joining.borrow_mut() = Some(path.clone());
        self.report(None);
        self.render();
        let wifi = self.clone();
        glib::spawn_future_local(async move {
            let result = async { iwd::NetworkProxy::builder(&connection).path(path.as_str()).build().await?.connect().await };
            let result = result.await;
            wifi.joining.borrow_mut().take();
            wifi.clear_passphrase();
            match result {
                Ok(()) => {
                    wifi.asking.borrow_mut().take();
                    wifi.report(None);
                }
                Err(error) => wifi.report(Some(joining_failed(&name, passphrase.is_some(), error))),
            }
            wifi.render();
        });
    }

    fn clear_passphrase(&self) {
        if let Some(slot) = self.passphrase.borrow().as_ref()
            && let Ok(mut slot) = slot.lock()
        {
            slot.take();
        }
    }

    /// A dialog for a network that doesn't broadcast its name: the name, and its password if it has one.
    fn join_hidden(self: &Rc<Self>, parent: Option<&gtk::Window>) {
        let Some(device) = self.snapshot.borrow().device.clone() else { return };
        let window = gtk::Window::builder().title("Join a hidden network").modal(true).resizable(false).default_width(380).build();
        window.set_transient_for(parent);
        window.set_application(parent.and_then(|parent| parent.application()).as_ref());
        window.add_css_class("settings");
        window.add_css_class("dialog");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.add_css_class("page");
        let name = gtk::Entry::builder().placeholder_text("Network name").build();
        name.add_css_class("field");
        name.update_property(&[gtk::accessible::Property::Label("Network name")]);
        column.append(&name);
        let password = gtk::PasswordEntry::builder().placeholder_text("Password, if it has one").show_peek_icon(true).build();
        password.add_css_class("field");
        password.update_property(&[gtk::accessible::Property::Label("Password")]);
        column.append(&password);
        let problem = ui::note("");
        problem.add_css_class("error");
        problem.set_wrap(true);
        problem.set_visible(false);
        column.append(&problem);
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
        let join = ui::button("Join", |_| {});
        join.add_css_class("suggested");
        buttons.append(&cancel);
        buttons.append(&join);
        column.append(&buttons);
        window.set_child(Some(&column));

        let submit = {
            let (wifi, window, name, password, problem, join) = (self.clone(), window.downgrade(), name.clone(), password.clone(), problem.clone(), join.clone());
            move || {
                let ssid = name.text().trim().to_owned();
                if ssid.is_empty() {
                    problem.set_label("Type the network’s name.");
                    problem.set_visible(true);
                    return;
                }
                let Some(connection) = wifi.connection() else { return };
                let typed = Some(Zeroizing::new(password.text().to_string())).filter(|typed| !typed.is_empty());
                if let Some(slot) = wifi.passphrase.borrow().as_ref()
                    && let Ok(mut slot) = slot.lock()
                {
                    *slot = typed.clone();
                }
                join.set_sensitive(false);
                problem.set_visible(false);
                let (wifi, window, problem, join, path) = (wifi.clone(), window.clone(), problem.clone(), join.clone(), device.path.clone());
                glib::spawn_future_local(async move {
                    let result = async { iwd::StationProxy::builder(&connection).path(path).build().await?.connect_hidden_network(&ssid).await };
                    let result = result.await;
                    wifi.clear_passphrase();
                    join.set_sensitive(true);
                    match result {
                        Ok(()) => {
                            if let Some(window) = window.upgrade() {
                                window.close();
                            }
                        }
                        Err(error) => {
                            problem.set_label(&joining_failed(&ssid, typed.is_some(), error));
                            problem.set_visible(true);
                        }
                    }
                });
            }
        };
        let submit = Rc::new(submit);
        join.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        password.connect_activate({
            let submit = submit.clone();
            move |_| submit()
        });
        name.connect_activate(move |_| submit());
        window.present();
    }
}

/// Why joining failed, in words: iwd says little more than that it did.
fn joining_failed(name: &str, with_password: bool, error: zbus::Error) -> String {
    match &error {
        zbus::Error::MethodError(kind, ..) => match kind.as_str() {
            "net.connman.iwd.Failed" | "net.connman.iwd.InvalidFormat" if with_password => format!("Couldn’t join {name}. Check the password."),
            "net.connman.iwd.Failed" => format!("Couldn’t join {name}."),
            "net.connman.iwd.Aborted" | "net.connman.iwd.Agent.Error.Canceled" => format!("Joining {name} was cancelled."),
            "net.connman.iwd.NotFound" => format!("{name} isn’t in range."),
            "net.connman.iwd.NotSupported" => format!("{name} uses security iwd can’t join from here."),
            _ => format!("Couldn’t join {name}: {}", bus::message(error, iwd::DAEMON)),
        },
        _ => format!("Couldn’t join {name}: {}", bus::message(error, iwd::DAEMON)),
    }
}

/// The accent tile with the Wi-Fi fan, for the network it's on.
fn tile() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(32).content_height(32).valign(gtk::Align::Center).build();
    area.set_draw_func(|_, cr, width, height| {
        let look = theme::look();
        theme::set_source(cr, &look.palette.accent, 1.0);
        let (width, height) = (f64::from(width), f64::from(height));
        let radius = 8.0;
        cr.new_sub_path();
        cr.arc(width - radius, radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(width - radius, height - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(radius, height - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(radius, radius, radius, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.close_path();
        cr.fill().ok();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.translate(7.0, 7.0);
        for arc in ARCS {
            icon::stroke(cr, arc, 18.0);
        }
        icon::stroke(cr, DOT, 18.0);
    });
    theme::redraw_on_change(&area);
    area
}

/// The Wi-Fi fan with `bars` of three lit, or all faint when out of range.
fn fan(bars: Option<u8>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(18).content_height(18).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, _, _| {
        let look = theme::look();
        let lit = bars.unwrap_or(0);
        for (index, arc) in ARCS.iter().enumerate() {
            theme::set_source(cr, &look.palette.text, if (index as u8) < lit { 1.0 } else { 0.22 });
            icon::stroke(cr, arc, 18.0);
        }
        theme::set_source(cr, &look.palette.text, if bars.is_some() { 1.0 } else { 0.22 });
        icon::stroke(cr, DOT, 18.0);
    });
    theme::redraw_on_change(&area);
    area.update_property(&[gtk::accessible::Property::Label(&match bars {
        Some(bars) => format!("Signal {bars} of 3"),
        None => "Not in range".to_owned(),
    })]);
    area
}

/// The network's name with a lock if it's secured, and a note under it.
fn name(text: &str, secured: bool, note: Option<&str>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_hexpand(true);
    column.set_valign(gtk::Align::Center);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("row-label");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&label);
    if secured {
        let lock = gtk::DrawingArea::builder().content_width(12).content_height(12).valign(gtk::Align::Center).build();
        lock.set_draw_func(|_, cr, _, _| {
            theme::set_source(cr, &theme::look().palette.text2, 1.0);
            icon::stroke(cr, LOCK, 12.0);
        });
        theme::redraw_on_change(&lock);
        lock.update_property(&[gtk::accessible::Property::Label("Secured")]);
        line.append(&lock);
    }
    column.append(&line);
    if let Some(note) = note {
        column.append(&ui::note(note));
    }
    column
}

fn texts(title: &str, note: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_hexpand(true);
    column.set_valign(gtk::Align::Center);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("row-label");
    label.add_css_class("strong");
    label.set_xalign(0.0);
    column.append(&label);
    let note = ui::note(note);
    note.set_xalign(0.0);
    column.append(&note);
    column
}
