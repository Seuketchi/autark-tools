//! The Hardware window: what needs attention first, then the laptop's controls and gauges, then every device.

use std::cell::RefCell;
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use autark_common::slider::Slider;
use autark_hw::controls::{self, Controls};
use autark_hw::devices::Class;
use autark_hw::doctor::{self, Finding, Machine, Status};
use autark_hw::{aliases, devices, kernel_log};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

/// Everything the scan finds, read off the main thread.
struct Snapshot {
    kernel: String,
    findings: Vec<Finding>,
    loose: Vec<kernel_log::Note>,
    lighting: Vec<String>,
}

fn kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|text| text.trim().to_owned()).unwrap_or_default()
}

fn snapshot() -> Snapshot {
    let kernel = kernel();
    let devices = devices::read();
    let aliases = aliases::load(&kernel);
    let loaded = doctor::loaded_modules();
    let notes = kernel_log::read();
    let findings = doctor::diagnose(&Machine { devices: &devices, aliases: &aliases, loaded: &loaded, notes: &notes, firmware_root: Path::new("/lib/firmware") });
    let loose = doctor::loose_notes(&findings, &notes, &doctor::loaded_modules()).into_iter().cloned().collect();
    let lighting = doctor::lighting_devices(&findings).into_iter().map(|device| device.title()).collect();
    Snapshot { kernel, findings, loose, lighting }
}

/// Sets a backlight or LED through logind, which lets the person at the keyboard do it without root.
fn set_level(subsystem: &str, name: &str, value: u32) {
    let result = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).and_then(|bus| {
        bus.call_sync(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1/session/auto",
            "org.freedesktop.login1.Session",
            "SetBrightness",
            Some(&(subsystem, name, value).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
    });
    if let Err(error) = result {
        eprintln!("hardware: setting {name}: {error}");
    }
}

/// Calls the root helper (see `autark-hw-helper`), which polkit lets act for the person at the keyboard. When it isn't
/// installed that's said plainly, as that's the likeliest reason it fails.
fn helper(method: &str, arguments: glib::Variant) -> Result<(), String> {
    let bus = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).map_err(|error| error.message().to_owned())?;
    // A password may be asked for, so there's time to type it.
    bus.call_sync(Some("org.autark.Hardware1"), "/org/autark/Hardware1", "org.autark.Hardware1", method, Some(&arguments), None, gio::DBusCallFlags::NONE, 120_000, gio::Cancellable::NONE)
        .map(drop)
        .map_err(|error| {
            let message = error.message().to_owned();
            if message.contains("ServiceUnknown") || message.contains("not provided by any .service") {
                "The hardware helper isn’t installed. It needs root once: see hardware/README.md.".to_owned()
            } else {
                message.rsplit_once(": ").map_or(message.clone(), |(_, rest)| rest.to_owned())
            }
        })
}

/// Sets a sysfs setting: directly if this user may, and through the helper if not.
fn apply_setting(path: &std::path::Path, value: &str) -> Result<(), String> {
    match controls::write_value(path, value) {
        Ok(()) => Ok(()),
        Err(_) => helper("WriteSetting", (path.to_string_lossy().as_ref(), value).to_variant()),
    }
}

fn card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card
}

fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text.to_uppercase()).xalign(0.0).build();
    label.add_css_class("heading");
    label
}

/// A row: a title and what to know about it on the left, a control or a value on the right.
fn line(title: &str, detail: &str, trailing: Option<&gtk::Widget>) -> gtk::Box {
    let name = gtk::Label::builder().label(title).xalign(0.0).build();
    name.add_css_class("name");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&name);
    if !detail.is_empty() {
        let note = gtk::Label::builder().label(detail).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build();
        note.add_css_class("detail");
        text.append(&note);
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("line");
    row.append(&text);
    if let Some(widget) = trailing {
        widget.set_valign(gtk::Align::Center);
        row.append(widget);
    }
    row
}

fn chip(text: &str, style: &str) -> gtk::Label {
    let chip = gtk::Label::new(Some(text));
    chip.add_css_class("chip");
    chip.add_css_class(style);
    chip.set_valign(gtk::Align::Center);
    chip
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

struct Hardware {
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    subtitle: gtk::Label,
    rescan: gtk::Button,
    attention: gtk::Box,
    laptop: gtk::Box,
    /// What happened to the last thing changed, when it didn't work.
    message: gtk::Label,
    live: gtk::Box,
    radios: gtk::Box,
    list: gtk::Box,
    /// What the controls last said, and what the lighting hint says, for the live updates.
    controls: RefCell<Controls>,
}

fn status_chip(finding: &Finding) -> gtk::Label {
    if !finding.missing_firmware.is_empty() {
        return chip("Needs firmware", "warn");
    }
    match &finding.status {
        Status::NoDriver => chip("No driver", "bad"),
        Status::Loadable(_) => chip("Driver not loaded", "warn"),
        Status::Declined(_) => chip("Not supported", "warn"),
        _ if !finding.unsupported.is_empty() => chip("Not supported", "warn"),
        Status::Working => chip("Working", "ok"),
        Status::Quiet => chip("Not needed", "quiet"),
    }
}

impl Hardware {
    fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Hardware").default_width(760).default_height(780).build();
        window.add_css_class("hw");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        let title = gtk::Label::builder().label("Hardware").xalign(0.0).build();
        title.add_css_class("page-title");
        let subtitle = gtk::Label::builder().label("Scanning…").xalign(0.0).build();
        subtitle.add_css_class("page-sub");
        let names = gtk::Box::new(gtk::Orientation::Vertical, 2);
        names.set_hexpand(true);
        names.append(&title);
        names.append(&subtitle);
        let rescan = gtk::Button::with_label("Rescan");
        rescan.add_css_class("plain");
        rescan.set_valign(gtk::Align::Center);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        top.add_css_class("top");
        top.append(&names);
        top.append(&rescan);
        top.append(&autark_common::chrome::controls(true));
        let handle = gtk::WindowHandle::builder().child(&top).build();

        let (attention, laptop, live, radios, list) = (gtk::Box::new(gtk::Orientation::Vertical, 10), card(), card(), card(), gtk::Box::new(gtk::Orientation::Vertical, 0));
        let message = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        message.add_css_class("message");
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.add_css_class("page");
        page.append(&attention);
        page.append(&heading("Laptop"));
        page.append(&laptop);
        page.append(&message);
        page.append(&live);
        page.append(&radios);
        page.append(&heading("Devices"));
        page.append(&list);
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&page).build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&handle);
        content.append(&scroller);
        window.set_child(Some(&content));

        let me = Rc::new(Self { window, title, subtitle, rescan, attention, laptop, message, live, radios, list, controls: RefCell::default() });
        me.rescan.connect_clicked({
            let me = Rc::downgrade(&me);
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.scan();
                }
            }
        });
        // Gauges move: fans, temperatures and the battery are looked at again every few seconds.
        glib::timeout_add_local(Duration::from_secs(3), {
            let me = Rc::downgrade(&me);
            move || match me.upgrade() {
                Some(me) => {
                    me.refresh_live();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        });
        let keep = me.clone();
        me.window.connect_close_request(move |_| {
            let _ = &keep;
            glib::Propagation::Proceed
        });
        me.scan();
        me
    }

    /// Shows what went wrong with the last thing changed.
    fn say(&self, text: &str) {
        self.message.set_text(text);
        self.message.set_visible(true);
    }

    /// Looks at the whole machine off the main thread, and shows it.
    fn scan(self: &Rc<Self>) {
        self.rescan.set_sensitive(false);
        self.subtitle.set_text("Scanning…");
        let me = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(shot) = gio::spawn_blocking(snapshot).await {
                me.show(&shot);
            }
            me.rescan.set_sensitive(true);
        });
    }

    fn show(self: &Rc<Self>, shot: &Snapshot) {
        self.message.set_visible(false);
        let controls = controls::detect(Path::new("/sys"));
        let problems: Vec<&Finding> = shot.findings.iter().filter(|finding| finding.is_problem()).collect();
        self.title.set_text(&format!("{} {}", controls.vendor, controls.product).trim().replace("Micro-Star International Co., Ltd.", "MSI"));
        let count = problems.len() + shot.loose.len();
        self.subtitle.set_text(&format!(
            "Linux {} · {} devices · {}",
            shot.kernel,
            shot.findings.len(),
            if count == 0 { "everything that needs a driver has one".to_owned() } else { format!("{count} {} attention", if count == 1 { "thing needs" } else { "things need" }) }
        ));

        // What needs attention.
        clear(&self.attention);
        if count > 0 {
            self.attention.append(&heading("Needs attention"));
        }
        for finding in problems {
            let item = card();
            let mut text = doctor::advice(finding).unwrap_or_default();
            for note in &finding.unsupported {
                text.push_str(&format!("\nKernel: {note}"));
            }
            let title = line(&finding.device.title(), text.trim(), Some(status_chip(finding).upcast_ref()));
            item.append(&title);
            if let Status::Loadable(modules) = &finding.status {
                let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                buttons.add_css_class("actions");
                for module in modules {
                    let button = gtk::Button::with_label(&format!("Load {module}"));
                    button.add_css_class("plain");
                    let (me, module) = (Rc::downgrade(self), module.clone());
                    button.connect_clicked(move |button| {
                        let Some(me) = me.upgrade() else { return };
                        button.set_sensitive(false);
                        match helper("LoadModule", (module.as_str(),).to_variant()) {
                            Ok(()) => me.scan(),
                            Err(error) => {
                                me.say(&format!("Couldn’t load {module}: {error}"));
                                button.set_sensitive(true);
                            }
                        }
                    });
                    buttons.append(&button);
                }
                item.append(&buttons);
            }
            self.attention.append(&item);
        }
        for note in &shot.loose {
            let item = card();
            let text = format!(
                "The kernel has a driver called {} for this machine, but it doesn’t support this model. It’s the part that normally runs the fans, the keyboard light and the battery limit.\nKernel: {}",
                note.driver, note.text
            );
            item.append(&line(&format!("Laptop controller ({})", note.driver), &text, Some(chip("Not supported", "warn").upcast_ref())));
            self.attention.append(&item);
        }

        // The controls that are fixed for now.
        clear(&self.laptop);
        for slider in &controls.brightness {
            self.laptop.append(&slider_line(&slider.label, slider.subsystem, &slider.name, slider.value, slider.max));
        }
        for slider in &controls.lights {
            self.laptop.append(&slider_line(&slider.label, slider.subsystem, &slider.name, slider.value, slider.max));
        }
        if controls.lights.is_empty() {
            let mut why = "The kernel doesn’t offer one for this laptop.".to_owned();
            if !shot.lighting.is_empty() {
                why.push_str(&format!("\nThe lighting here is a separate controller ({}), which needs a driver profile that hasn’t been written for it yet.", shot.lighting.join(", ")));
            }
            self.laptop.append(&line("Keyboard backlight", &why, None));
        }
        match &controls.profile {
            Some(profile) => {
                let choices = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                for choice in &profile.choices {
                    let button = gtk::ToggleButton::with_label(choice);
                    button.add_css_class("choice");
                    button.set_active(*choice == profile.current);
                    let (me, path, value) = (Rc::downgrade(self), profile.path.clone(), choice.clone());
                    button.connect_clicked(move |_| {
                        if let (Some(me), Err(error)) = (me.upgrade(), apply_setting(&path, &value)) {
                            me.say(&format!("Couldn’t change the power profile: {error}"));
                        }
                    });
                    choices.append(&button);
                }
                self.laptop.append(&line("Power profile", "How hard the machine works against how hot and loud it gets.", Some(choices.upcast_ref())));
            }
            None => self.laptop.append(&line("Power profile", "The kernel doesn’t offer one for this laptop.", None)),
        }
        for mode in &controls.modes {
            let choices = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            let mut group: Option<gtk::ToggleButton> = None;
            for choice in &mode.choices {
                let button = gtk::ToggleButton::with_label(choice);
                button.add_css_class("choice");
                button.set_active(*choice == mode.current);
                button.set_group(group.as_ref());
                group.get_or_insert(button.clone());
                let (me, path, value, label) = (Rc::downgrade(self), mode.path.clone(), choice.clone(), mode.label);
                button.connect_toggled(move |button| {
                    if !button.is_active() {
                        return;
                    }
                    if let (Some(me), Err(error)) = (me.upgrade(), apply_setting(&path, &value)) {
                        me.say(&format!("Couldn’t change {}: {error}", label.to_lowercase()));
                    }
                });
                choices.append(&button);
            }
            self.laptop.append(&line(mode.label, mode.note, Some(choices.upcast_ref())));
        }
        for battery in &controls.batteries {
            match &battery.limit {
                Some(limit) => {
                    let slider = Slider::new(f64::from(limit.percent.saturating_sub(50)) / 50.0);
                    slider.widget.set_size_request(180, 20);
                    let (me, path) = (Rc::downgrade(self), limit.path.clone());
                    // Written when the slider is let go, not at every step of the drag: each write may ask for a password.
                    let pending = Rc::new(std::cell::Cell::new(None::<u32>));
                    slider.connect_changed({
                        let pending = pending.clone();
                        move |fraction| pending.set(Some(50 + (fraction * 50.0).round() as u32))
                    });
                    let drag = gtk::GestureClick::new();
                    drag.connect_released(move |_, _, _, _| {
                        if let (Some(percent), Some(me)) = (pending.take(), me.upgrade())
                            && let Err(error) = apply_setting(&path, &percent.to_string())
                        {
                            me.say(&format!("Couldn’t set the charge limit: {error}"));
                        }
                    });
                    slider.widget.add_controller(drag);
                    let widget = slider.widget.clone();
                    // The slider's handlers live as long as its widget does.
                    std::mem::forget(slider);
                    self.laptop.append(&line("Charge limit", &format!("Stopping short of full makes a battery last longer. It’s {}% now.", limit.percent), Some(widget.upcast_ref())));
                }
                None => self.laptop.append(&line("Charge limit", "The kernel doesn’t offer one for this laptop.", None)),
            }
        }

        // Radios.
        clear(&self.radios);
        for radio in &controls.radios {
            let switch = gtk::Switch::builder().active(!radio.soft_blocked && !radio.hard_blocked).sensitive(!radio.hard_blocked).build();
            let index = radio.index;
            switch.connect_state_set(move |_, on| {
                if let Err(error) = controls::set_radio(index, !on) {
                    eprintln!("hardware: radio {index}: {error}");
                }
                glib::Propagation::Proceed
            });
            let name = match radio.kind.as_str() {
                "wlan" => "Wi-Fi",
                "bluetooth" => "Bluetooth",
                "wwan" => "Mobile data",
                other => other,
            };
            let detail = if radio.hard_blocked { "Turned off by a switch on the laptop.".to_owned() } else { radio.name.clone() };
            self.radios.append(&line(name, &detail, Some(switch.upcast_ref())));
        }
        self.radios.set_visible(!controls.radios.is_empty());
        *self.controls.borrow_mut() = controls;
        self.refresh_live();

        // Every device, by kind.
        clear(&self.list);
        let mut shown: Option<Class> = None;
        let mut group = card();
        for finding in &shot.findings {
            if shown != Some(finding.device.class) {
                if shown.is_some() {
                    self.list.append(&group);
                }
                shown = Some(finding.device.class);
                group = card();
                let title = gtk::Label::builder().label(finding.device.class.label()).xalign(0.0).build();
                title.add_css_class("group");
                group.append(&title);
            }
            let drivers = if finding.device.drivers.is_empty() { "no driver".to_owned() } else { finding.device.drivers.join(", ") };
            let row = line(&finding.device.title(), &format!("{} · {drivers}", finding.device.id), Some(status_chip(finding).upcast_ref()));
            group.append(&row);
        }
        if shown.is_some() {
            self.list.append(&group);
        }
    }

    /// The gauges: battery, fans and temperatures as they are now.
    fn refresh_live(&self) {
        let now = controls::detect(Path::new("/sys"));
        clear(&self.live);
        for battery in &now.batteries {
            let mut parts = vec![format!("{}%", battery.percent), battery.status.to_lowercase()];
            if let Some(health) = battery.health {
                parts.push(format!("{health}% of its original capacity"));
            }
            if let Some(cycles) = battery.cycles {
                parts.push(format!("{cycles} charge cycles"));
            }
            self.live.append(&line("Battery", &parts.join(" · "), None));
        }
        let gauges: Vec<String> = now.temperatures.iter().chain(&now.fans).map(|reading| format!("{} {}", reading.label, reading.value)).collect();
        if !gauges.is_empty() {
            self.live.append(&line("Cooling and temperature", &gauges.join(" · "), None));
        }
        self.live.set_visible(self.live.first_child().is_some());
    }
}

/// A slider that sets a backlight or LED as you drag, and says where it is.
fn slider_line(label: &str, subsystem: &'static str, name: &str, value: u32, max: u32) -> gtk::Box {
    let slider = Slider::new(f64::from(value) / f64::from(max));
    slider.widget.set_size_request(180, 20);
    let shown = gtk::Label::new(Some(&format!("{}%", (f64::from(value) / f64::from(max) * 100.0).round())));
    shown.add_css_class("detail");
    shown.set_width_chars(4);
    let (shown_label, name) = (shown.clone(), name.to_owned());
    slider.connect_changed(move |fraction| {
        // A screen never goes fully dark: at nothing it looks off, and the keyboard's levels are few.
        let raw = if subsystem == "backlight" { ((f64::from(max) * fraction).round() as u32).max(max / 20).max(1) } else { (f64::from(max) * fraction).round() as u32 };
        set_level(subsystem, &name, raw);
        shown_label.set_text(&format!("{}%", (f64::from(raw) / f64::from(max) * 100.0).round()));
    });
    let both = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    both.append(slider.widget.upcast_ref::<gtk::Widget>());
    both.append(&shown);
    // The slider's handlers live as long as its widget does.
    std::mem::forget(slider);
    line(label, "", Some(both.upcast_ref()))
}

pub fn run() -> ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Hardware").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("hw.css")));
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        Hardware::new(app).window.present();
    });
    let code = app.run_with_args::<&str>(&[]);
    ExitCode::from(code.get())
}
