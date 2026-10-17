//! Sound: the default output's volume and mute, which output and input to use, the input's volume and live
//! level, all from PipeWire through `autark_common::audio` and changed through `wpctl`, as the bar does. Sound
//! effects are `sound.toml`'s `effects`, which the tools read each time they'd play one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use autark_common::audio::{self, Device, Snapshot, wpctl};
use autark_common::theme;
use gtk::glib;
use gtk::prelude::*;

use crate::{config, ui};

/// The input level's segments, and how many are lit before the warning colour.
const SEGMENTS: usize = 12;
const SAFE: usize = 9;

struct Sound {
    column: gtk::Box,
    volume: gtk::Scale,
    volume_value: gtk::Label,
    mute: gtk::Switch,
    outputs: gtk::Box,
    inputs: gtk::Box,
    /// The input rows, which go above the level row in the same group.
    input_rows: RefCell<Vec<gtk::Widget>>,
    mic: gtk::Scale,
    level: gtk::DrawingArea,
    /// The input level as drawn, from 0 to 1, falling back slowly.
    shown_level: Cell<f64>,
    snapshot: RefCell<Snapshot>,
    /// Set while the controls are moved to match PipeWire, so that isn't sent back as a change.
    syncing: Cell<bool>,
    /// Dragging sends a value every few milliseconds; only the latest is applied, at most every 60 ms.
    pending: Rc<Cell<Option<(u32, f64)>>>,
    meter: RefCell<Option<audio::Meter>>,
    unavailable: RefCell<Option<gtk::Label>>,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Sound", None));

    let volume = scale();
    let volume_value = gtk::Label::new(Some("0%"));
    volume_value.add_css_class("value");
    volume_value.set_width_chars(5);
    volume_value.set_xalign(1.0);
    let mic = scale();
    let level = gtk::DrawingArea::builder().content_width((SEGMENTS * 11 - 3) as i32).content_height(14).valign(gtk::Align::Center).build();
    level.update_property(&[gtk::accessible::Property::Label("Input level")]);
    let sound = Rc::new(Sound {
        column: column.clone(),
        volume,
        volume_value,
        mute: gtk::Switch::new(),
        outputs: ui::group(),
        inputs: ui::group(),
        input_rows: RefCell::default(),
        mic,
        level,
        shown_level: Cell::new(0.0),
        snapshot: RefCell::default(),
        syncing: Cell::new(false),
        pending: Rc::default(),
        meter: RefCell::default(),
        unavailable: RefCell::default(),
    });

    let group = ui::group();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.append(&sound.volume);
    line.append(&sound.volume_value);
    sound.volume.update_property(&[gtk::accessible::Property::Label("Output volume")]);
    group.append(&ui::row("Output volume", None, &line));
    sound.mute.set_valign(gtk::Align::Center);
    sound.mute.update_property(&[gtk::accessible::Property::Label("Mute")]);
    group.append(&ui::row("Mute", None, &sound.mute));
    let effects = config::get_bool(&config::read("sound"), "effects").unwrap_or(true);
    let status = ui::note("");
    status.add_css_class("error");
    status.set_visible(false);
    let effects = ui::switch(effects, {
        let status = status.clone();
        move |on| {
            let written = config::edit("sound", |document| config::set_in(document, "effects", on));
            status.set_visible(written.is_err());
            if let Err(error) = written {
                status.set_label(&error);
            }
        }
    });
    effects.update_property(&[gtk::accessible::Property::Label("Play sound effects")]);
    group.append(&ui::row("Play sound effects", Some("Alerts and the screenshot shutter"), &effects));
    column.append(&group);
    column.append(&ui::section("Output", None, &sound.outputs));

    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    line.append(&sound.level);
    sound.mic.update_property(&[gtk::accessible::Property::Label("Input volume")]);
    line.append(&sound.mic);
    sound.inputs.append(&ui::row("Input level", None, &line));
    column.append(&ui::section("Input", None, &sound.inputs));
    column.append(&status);

    sound.connect();
    // Listens to the microphone only while the pane is on screen.
    page.connect_map({
        let sound = Rc::downgrade(&sound);
        move |_| {
            if let Some(sound) = sound.upgrade() {
                sound.listen();
            }
        }
    });
    page.connect_unmap({
        let sound = Rc::downgrade(&sound);
        move |_| {
            if let Some(sound) = sound.upgrade() {
                sound.meter.borrow_mut().take();
                sound.shown_level.set(0.0);
                sound.level.queue_draw();
            }
        }
    });
    let received = audio::spawn();
    glib::spawn_future_local({
        let sound = sound.clone();
        async move {
            while let Ok(snapshot) = received.recv().await {
                sound.show(snapshot);
            }
            sound.unavailable("PipeWire isn’t running, so there’s no sound to set.");
        }
    });
    page.upcast()
}

fn scale() -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_width_request(220);
    scale.set_draw_value(false);
    scale.set_valign(gtk::Align::Center);
    scale
}

impl Sound {
    fn connect(self: &Rc<Self>) {
        for (scale, output) in [(&self.volume, true), (&self.mic, false)] {
            let sound = Rc::downgrade(self);
            scale.connect_value_changed(move |scale| {
                let Some(sound) = sound.upgrade() else { return };
                if output {
                    sound.volume_value.set_label(&format!("{:.0}%", scale.value()));
                }
                if sound.syncing.get() {
                    return;
                }
                let snapshot = sound.snapshot.borrow();
                let Some(id) = (if output { snapshot.default_output } else { snapshot.default_input }) else { return };
                if sound.pending.replace(Some((id, scale.value()))).is_some() {
                    return;
                }
                let pending = sound.pending.clone();
                glib::timeout_add_local_once(Duration::from_millis(60), move || {
                    if let Some((id, value)) = pending.take() {
                        wpctl(&["set-volume", &id.to_string(), &format!("{:.2}", value / 100.0)]);
                    }
                });
            });
        }
        let sound = Rc::downgrade(self);
        self.mute.connect_state_set(move |_, on| {
            if let Some(sound) = sound.upgrade()
                && !sound.syncing.get()
                && let Some(id) = sound.snapshot.borrow().default_output
            {
                wpctl(&["set-mute", &id.to_string(), if on { "1" } else { "0" }]);
            }
            glib::Propagation::Proceed
        });
        let sound = Rc::downgrade(self);
        self.level.set_draw_func(move |_, cr, _, height| {
            let Some(sound) = sound.upgrade() else { return };
            let look = theme::look();
            let lit = (sound.shown_level.get() * SEGMENTS as f64).round() as usize;
            for index in 0..SEGMENTS {
                let (colour, alpha) = match index {
                    index if index >= lit => (&look.palette.text, 0.12),
                    index if index < SAFE => (&look.palette.accent, 1.0),
                    _ => (&look.palette.warning, 1.0),
                };
                theme::set_source(cr, colour, alpha);
                rounded(cr, index as f64 * 11.0, 0.0, 8.0, f64::from(height), 2.0);
                cr.fill().ok();
            }
        });
        theme::redraw_on_change(&self.level);
    }

    fn show(self: &Rc<Self>, snapshot: Snapshot) {
        if let Some(label) = self.unavailable.borrow_mut().take() {
            self.column.remove(&label);
        }
        let before = self.snapshot.replace(snapshot.clone());
        self.syncing.set(true);
        let output = snapshot.output().and_then(|device| device.level);
        self.volume.set_sensitive(output.is_some());
        self.mute.set_sensitive(output.is_some());
        if let Some(level) = output
            && self.pending.get().is_none()
        {
            self.volume.set_value(f64::from(level.volume) * 100.0);
            self.mute.set_active(level.muted);
        }
        let input = snapshot.input().and_then(|device| device.level);
        self.mic.set_sensitive(input.is_some());
        if let Some(level) = input
            && self.pending.get().is_none()
        {
            self.mic.set_value(f64::from(level.volume) * 100.0);
        }
        self.syncing.set(false);

        let names = |devices: &[Device]| devices.iter().map(|device| (device.id, device.nick.clone(), device.kind)).collect::<Vec<_>>();
        if (names(&snapshot.outputs), snapshot.default_output) != (names(&before.outputs), before.default_output) || before == Snapshot::default() {
            ui::clear(&self.outputs);
            for row in rows(&snapshot.outputs, snapshot.default_output, false, "No outputs") {
                self.outputs.append(&row);
            }
        }
        if (names(&snapshot.inputs), snapshot.default_input) != (names(&before.inputs), before.default_input) || before == Snapshot::default() {
            for row in self.input_rows.take() {
                self.inputs.remove(&row);
            }
            let rows = rows(&snapshot.inputs, snapshot.default_input, true, "No inputs");
            for row in rows.iter().rev() {
                self.inputs.prepend(row);
            }
            *self.input_rows.borrow_mut() = rows;
            // A different microphone: listen to that one instead.
            if before.default_input != snapshot.default_input {
                self.meter.borrow_mut().take();
            }
        }
        if self.level.is_mapped() {
            self.listen();
        }
    }

    /// Starts showing the default input's level.
    fn listen(self: &Rc<Self>) {
        if self.meter.borrow().is_some() || self.snapshot.borrow().default_input.is_none() {
            return;
        }
        let meter = audio::meter();
        let peaks = meter.peaks.clone();
        *self.meter.borrow_mut() = Some(meter);
        let sound = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(peak) = peaks.recv().await {
                let Some(sound) = sound.upgrade() else { return };
                // Shown in decibels, the bottom of the scale 60 dB down, as level meters are.
                let level = ((20.0 * f64::from(peak.max(1e-6)).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
                sound.shown_level.set(level.max(sound.shown_level.get() * 0.85));
                sound.level.queue_draw();
            }
        });
    }

    fn unavailable(&self, text: &str) {
        if self.unavailable.borrow().is_some() {
            return;
        }
        let label = ui::note(text);
        label.add_css_class("unavailable");
        label.set_wrap(true);
        self.column.insert_child_after(&label, self.column.first_child().as_ref());
        *self.unavailable.borrow_mut() = Some(label);
        for widget in [self.volume.upcast_ref::<gtk::Widget>(), self.mute.upcast_ref(), self.mic.upcast_ref()] {
            widget.set_sensitive(false);
        }
    }
}

/// A radio row per device, the default ticked; picking one makes it the default.
fn rows(devices: &[Device], default: Option<u32>, input: bool, none: &str) -> Vec<gtk::Widget> {
    if devices.is_empty() {
        return vec![ui::row(none, None, &gtk::Box::new(gtk::Orientation::Horizontal, 0)).upcast()];
    }
    devices
        .iter()
        .map(|device| {
            let chosen = Some(device.id) == default;
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            let mark = gtk::Label::new(Some(if chosen { "✓" } else { "" }));
            mark.add_css_class("mark");
            mark.set_width_chars(2);
            line.append(&mark);
            let name = gtk::Label::new(Some(&device.nick));
            name.add_css_class("row-label");
            name.set_xalign(0.0);
            name.set_hexpand(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            line.append(&name);
            let kind = if input && device.kind == "Bluetooth" { "Bluetooth headset" } else { device.kind };
            line.append(&ui::note(kind));
            let button = gtk::Button::builder().child(&line).css_classes(["row", "pick"]).build();
            button.update_property(&[gtk::accessible::Property::Label(&device.nick)]);
            button.update_state(&[gtk::accessible::State::Checked(if chosen { gtk::AccessibleTristate::True } else { gtk::AccessibleTristate::False })]);
            let id = device.id.to_string();
            button.connect_clicked(move |_| wpctl(&["set-default", &id]));
            button.upcast()
        })
        .collect()
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, PI + FRAC_PI_2);
    cr.close_path();
}
