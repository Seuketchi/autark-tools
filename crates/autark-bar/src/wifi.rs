//! Wi-Fi: iwd over D-Bus (Autark's stack), else NetworkManager; hidden without a Wi-Fi device.
//! Clicking opens the network list: turn Wi-Fi on or off, and join a network.

mod iwd;
mod network_manager;

use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

use futures_util::StreamExt;
use futures_util::stream::{Fuse, LocalBoxStream};
use gtk::cairo::{Context, LineCap};
use gtk::glib;
use gtk::prelude::*;

use crate::menu;
use crate::shared::Shared;

#[derive(Clone, Default, PartialEq)]
pub enum Wifi {
    #[default]
    Unavailable,
    Off,
    Disconnected,
    Connected { name: String, bars: u8 },
}

/// One network in the list. `id` is whatever the backend needs to join it.
#[derive(Clone, PartialEq)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub bars: u8,
    pub secured: bool,
    pub known: bool,
    pub connected: bool,
}

pub enum Update {
    Status(Wifi),
    Networks(Vec<Network>),
    /// Joining this network failed.
    Failed(String),
}

pub enum Command {
    Scan,
    Connect { id: String, passphrase: Option<String> },
    SetPowered(bool),
}

/// A change notification the backends wait on; `never()` stands in when there's nothing to watch.
type Changes = Fuse<LocalBoxStream<'static, ()>>;

fn changes<T: 'static>(stream: impl futures_util::Stream<Item = T> + 'static) -> Changes {
    stream.map(|_| ()).boxed_local().fuse()
}

fn never() -> Changes {
    changes(futures_util::stream::pending::<()>())
}

/// What every bar shows, kept by the one backend watcher.
#[derive(Clone, Default, PartialEq)]
pub struct State {
    status: Wifi,
    networks: Vec<Network>,
    /// The network that last failed to join, until something connects.
    failed: Option<String>,
}

#[derive(Clone)]
pub struct Source {
    state: Shared<State>,
    commands: async_channel::Sender<Command>,
}

/// Starts the backend once for every bar: iwd if it runs, else NetworkManager.
pub fn source() -> Source {
    let state = Shared::new(State::default());
    let (commands, received_commands) = async_channel::unbounded();
    let (updates, received) = async_channel::unbounded();
    glib::spawn_future_local(async move {
        match iwd::run(updates.clone(), received_commands.clone()).await {
            Ok(true) => {}
            Ok(false) => {
                if let Err(err) = network_manager::run(updates, received_commands).await {
                    eprintln!("wifi: NetworkManager: {err}");
                }
            }
            Err(err) => eprintln!("wifi: iwd: {err}"),
        }
    });
    glib::spawn_future_local({
        let state = state.clone();
        async move {
            let mut current = State::default();
            while let Ok(update) = received.recv().await {
                match update {
                    Update::Status(status) => {
                        if matches!(status, Wifi::Connected { .. }) {
                            current.failed = None;
                        }
                        current.status = status;
                    }
                    Update::Networks(networks) => current.networks = networks,
                    Update::Failed(name) => current.failed = Some(name),
                }
                state.set(current.clone());
            }
            // The backend has stopped, so what's shown can't be trusted.
            state.set(State::default());
        }
    });
    Source { state, commands }
}

pub fn view(source: &Source) -> gtk::MenuButton {
    let status = Rc::new(RefCell::new(Wifi::default()));
    let icon = fan(16, {
        let status = status.clone();
        move || match &*status.borrow() {
            Wifi::Connected { bars, .. } => *bars,
            _ => 0,
        }
    });

    let commands = source.commands.clone();
    let power = gtk::Switch::builder().valign(gtk::Align::Center).build();
    // Set while the switch is moved to match the backend, so that isn't sent back as a change.
    let syncing = Rc::new(std::cell::Cell::new(false));
    power.connect_state_set({
        let (commands, syncing) = (commands.clone(), syncing.clone());
        move |_, on| {
            if !syncing.get() {
                let _ = commands.try_send(Command::SetPowered(on));
            }
            glib::Propagation::Proceed
        }
    });
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let heading = menu::heading("Wi-Fi");
    heading.set_hexpand(true);
    header.append(&heading);
    header.append(&power);

    let message = menu::caption("");
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let networks = gtk::Box::new(gtk::Orientation::Vertical, 10);
    networks.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    networks.append(&menu::caption("Networks"));
    networks.append(&message);
    networks.append(&list);

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
    panel.append(&header);
    panel.append(&networks);

    let button = menu::button(&icon, &panel, &["icon"]);
    if let Some(popover) = button.popover() {
        let commands = commands.clone();
        popover.connect_show(move |_| {
            let _ = commands.try_send(Command::Scan);
        });
    }

    let join = Join::new(commands.clone());
    let shown = RefCell::new(None::<Vec<Network>>);
    source.state.watch(&button, move |button, state| {
        button.set_visible(state.status != Wifi::Unavailable);
        button.set_tooltip_text(Some(&match &state.status {
            Wifi::Connected { name, .. } => name.clone(),
            Wifi::Off => "Wi-Fi: off".to_owned(),
            _ => "Wi-Fi: not connected".to_owned(),
        }));
        syncing.set(true);
        power.set_active(state.status != Wifi::Off);
        syncing.set(false);
        networks.set_visible(state.status != Wifi::Off);
        message.set_visible(state.failed.is_some());
        if let Some(name) = &state.failed {
            message.set_label(&format!("Couldn't join {name}"));
        }
        *status.borrow_mut() = state.status.clone();
        icon.queue_draw();
        let mut shown = shown.borrow_mut();
        if shown.as_ref() != Some(&state.networks) {
            show_networks(&list, &state.networks, &join, &commands);
            *shown = Some(state.networks.clone());
        }
    });
    button
}

fn show_networks(list: &gtk::Box, found: &[Network], join: &Join, commands: &async_channel::Sender<Command>) {
    join.detach();
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for network in found {
        let row = network_row(network);
        list.append(&row);
        if join.is_for(&network.id) {
            join.show_after(&row, &network.id);
        }
        let (network, join, commands) = (network.clone(), join.clone(), commands.clone());
        row.connect_clicked(move |row| {
            if network.connected {
                return;
            }
            if network.secured && !network.known {
                join.show_after(row, &network.id);
            } else {
                let _ = commands.try_send(Command::Connect { id: network.id.clone(), passphrase: None });
            }
        });
    }
}

fn network_row(network: &Network) -> gtk::Button {
    let bars = network.bars;
    let label = gtk::Label::builder()
        .label(&network.name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(26)
        .build();
    let check = gtk::Label::new(Some("✓"));
    check.add_css_class("check");
    check.set_opacity(if network.connected { 1.0 } else { 0.0 });
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.append(&fan(14, move || bars));
    content.append(&label);
    if network.secured {
        content.append(&lock());
    }
    content.append(&check);
    let row = gtk::Button::builder().child(&content).build();
    row.add_css_class("row");
    row
}

/// The password field shown under a secured network that hasn't been joined before.
#[derive(Clone)]
struct Join {
    widget: gtk::Box,
    entry: gtk::PasswordEntry,
    network: Rc<RefCell<Option<String>>>,
}

impl Join {
    fn new(commands: async_channel::Sender<Command>) -> Self {
        let entry = gtk::PasswordEntry::builder().show_peek_icon(true).hexpand(true).build();
        entry.set_placeholder_text(Some("Password"));
        let button = gtk::Button::with_label("Join");
        button.add_css_class("join");
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        widget.add_css_class("join-row");
        widget.append(&entry);
        widget.append(&button);
        let join = Self { widget, entry, network: Rc::default() };
        let submit = {
            let join = join.clone();
            move || {
                let passphrase = join.entry.text().to_string();
                let Some(id) = join.network.borrow_mut().take() else {
                    return;
                };
                if !passphrase.is_empty() {
                    let _ = commands.try_send(Command::Connect { id, passphrase: Some(passphrase) });
                }
                join.entry.set_text("");
                join.detach();
            }
        };
        join.entry.connect_activate({
            let submit = submit.clone();
            move |_| submit()
        });
        button.connect_clicked(move |_| submit());
        join
    }

    fn is_for(&self, id: &str) -> bool {
        self.network.borrow().as_deref() == Some(id)
    }

    fn show_after(&self, row: &gtk::Button, id: &str) {
        self.detach();
        if !self.is_for(id) {
            self.entry.set_text("");
            *self.network.borrow_mut() = Some(id.to_owned());
        }
        if let Some(list) = row.parent().and_downcast::<gtk::Box>() {
            list.insert_child_after(&self.widget, Some(row));
            self.entry.grab_focus();
        }
    }

    /// Takes the field out of the list, keeping which network it was for.
    fn detach(&self) {
        if let Some(list) = self.widget.parent().and_downcast::<gtk::Box>() {
            list.remove(&self.widget);
        }
    }
}

/// The design's Wi-Fi fan: three arcs over a dot, lit from the inside out by signal strength.
fn fan(size: i32, bars: impl Fn() -> u8 + 'static) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::builder()
        .content_width(size)
        .content_height(size)
        .valign(gtk::Align::Center)
        .build();
    icon.set_draw_func(move |area, cr, width, height| draw_fan(area, cr, width.min(height), bars()));
    icon
}

fn draw_fan(area: &gtk::DrawingArea, cr: &Context, size: i32, bars: u8) {
    let lit = area.color();
    let dim = lit.with_alpha(lit.alpha() * 0.25);
    let scale = f64::from(size) / 24.0;
    cr.scale(scale, scale);
    cr.set_line_width(2.0);
    cr.set_line_cap(LineCap::Round);

    // Each arc spans from (12 - half, y) to (12 + half, y) on a circle of the given radius.
    for (index, (half, y, radius)) in [(3.5_f64, 15.5_f64, 5.0_f64), (7.0, 12.0, 10.0), (10.0, 8.5, 15.0)].into_iter().enumerate() {
        let centre = y + (radius * radius - half * half).sqrt();
        let start = (y - centre).atan2(-half);
        cr.new_sub_path();
        cr.arc(12.0, centre, radius, start, -PI - start);
        cr.set_source_color(if (index as u8) < bars { &lit } else { &dim });
        let _ = cr.stroke();
    }
    cr.arc(12.0, 19.0, 1.0, 0.0, 2.0 * PI);
    cr.set_source_color(if bars > 0 { &lit } else { &dim });
    let _ = cr.fill();
}

/// A small padlock beside secured networks.
fn lock() -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::builder()
        .content_width(12)
        .content_height(12)
        .valign(gtk::Align::Center)
        .build();
    icon.add_css_class("lock");
    icon.set_draw_func(|area, cr, width, height| {
        let scale = f64::from(width.min(height)) / 24.0;
        cr.scale(scale, scale);
        cr.set_source_color(&area.color());
        cr.set_line_width(2.4);
        cr.set_line_cap(LineCap::Round);
        cr.move_to(7.5, 11.0);
        cr.line_to(7.5, 8.5);
        cr.arc(12.0, 8.5, 4.5, PI, 2.0 * PI);
        cr.line_to(16.5, 11.0);
        let _ = cr.stroke();
        cr.rectangle(5.0, 11.0, 14.0, 10.0);
        let _ = cr.fill();
    });
    icon
}

/// iwd's signal in 100 × dBm, or NetworkManager's 0–100 strength, as one to three arcs.
fn bars_from_dbm(signal: i16) -> u8 {
    match signal {
        s if s >= -6000 => 3,
        s if s >= -7000 => 2,
        _ => 1,
    }
}

fn bars_from_strength(strength: u8) -> u8 {
    match strength {
        0..=33 => 1,
        34..=66 => 2,
        _ => 3,
    }
}

/// Connected first, then networks joined before, then the strongest; one row per name.
fn sorted(mut networks: Vec<Network>) -> Vec<Network> {
    networks.sort_by(|a, b| {
        (b.connected, b.known, b.bars)
            .cmp(&(a.connected, a.known, a.bars))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut seen = std::collections::HashSet::new();
    networks.retain(|network| seen.insert(network.name.clone()));
    networks
}
