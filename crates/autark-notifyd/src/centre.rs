//! The notification centre: a panel down the right edge with quick settings and every kept
//! notification. Opened from the bar; Escape closes it.

use std::f64::consts::PI;
use std::rc::Weak;

use gtk::cairo::Operator;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::{Daemon, WIDTH};

pub struct Centre {
    window: gtk::ApplicationWindow,
    list: gtk::Box,
    daemon: Weak<Daemon>,
}

impl Centre {
    pub fn new(app: &gtk::Application, daemon: Weak<Daemon>) -> Self {
        let window = gtk::ApplicationWindow::new(app);
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("autark-notification-centre"));
        for edge in [Edge::Top, Edge::Right, Edge::Bottom] {
            window.set_anchor(edge, true);
        }
        window.set_margin(Edge::Top, 12);
        window.set_margin(Edge::Right, 12);
        window.set_margin(Edge::Bottom, 12);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.set_default_size(WIDTH, -1);
        window.add_css_class("notifications");

        let escape = gtk::EventControllerKey::new();
        escape.connect_key_pressed({
            let window = window.downgrade();
            move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape
                    && let Some(window) = window.upgrade()
                {
                    window.set_visible(false);
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            }
        });
        window.add_controller(escape);

        let clear = gtk::Button::with_label("Clear all");
        clear.add_css_class("clear");
        clear.connect_clicked({
            let daemon = daemon.clone();
            move |_| {
                if let Some(daemon) = daemon.upgrade() {
                    daemon.clear();
                }
            }
        });
        let heading = gtk::Label::builder().label("Notifications").xalign(0.0).hexpand(true).build();
        heading.add_css_class("heading");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("header");
        header.append(&heading);
        header.append(&clear);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.append(&quick_settings(daemon.clone()));
        content.append(&header);
        content.append(&list);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .build();
        window.set_child(Some(&scroller));

        Self { window, list, daemon }
    }

    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    pub fn toggle(&self) {
        if self.window.is_visible() {
            self.window.set_visible(false);
        } else {
            self.window.set_visible(true);
            self.refresh();
        }
    }

    /// Rebuilds the list from what the daemon keeps; only while open, and on opening.
    pub fn refresh(&self) {
        if !self.window.is_visible() {
            return;
        }
        let Some(daemon) = self.daemon.upgrade() else {
            return;
        };
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let notifications = daemon.notifications();
        if notifications.is_empty() {
            let none = gtk::Label::new(Some("No notifications"));
            none.add_css_class("none");
            self.list.append(&none);
        }
        let handlers = daemon.handlers();
        for notification in &notifications {
            self.list.append(&crate::card::new(notification, &handlers));
        }
    }
}

/// The quick settings card. Focus (do not disturb) for now; Wi-Fi, Bluetooth, night light and
/// the sliders join it as their services are written.
fn quick_settings(daemon: Weak<Daemon>) -> gtk::Box {
    let state = gtk::Label::builder().label("Off").xalign(0.0).build();
    state.add_css_class("state");
    let label = gtk::Label::builder().label("Focus").xalign(0.0).build();
    label.add_css_class("label");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_valign(gtk::Align::Center);
    text.append(&label);
    text.append(&state);

    let moon = gtk::DrawingArea::builder().content_width(16).content_height(16).build();
    moon.set_draw_func(|area, cr, width, height| {
        // A crescent: a disc with an offset disc cut out of it.
        let scale = f64::from(width.min(height)) / 24.0;
        cr.scale(scale, scale);
        cr.set_source_color(&area.color());
        cr.arc(11.0, 13.0, 8.0, 0.0, 2.0 * PI);
        let _ = cr.fill();
        cr.set_operator(Operator::Clear);
        cr.arc(16.0, 8.0, 6.5, 0.0, 2.0 * PI);
        let _ = cr.fill();
    });
    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    badge.add_css_class("badge");
    badge.set_valign(gtk::Align::Center);
    badge.set_homogeneous(true);
    moon.set_halign(gtk::Align::Center);
    moon.set_valign(gtk::Align::Center);
    badge.append(&moon);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.append(&badge);
    content.append(&text);
    let focus = gtk::ToggleButton::builder().child(&content).build();
    focus.add_css_class("toggle");
    focus.connect_toggled(move |focus| {
        let on = focus.is_active();
        state.set_label(if on { "Do not disturb" } else { "Off" });
        if let Some(daemon) = daemon.upgrade() {
            daemon.do_not_disturb.set(on);
        }
    });

    let grid = gtk::Grid::builder().column_spacing(10).row_spacing(10).column_homogeneous(true).build();
    grid.attach(&focus, 0, 0, 1, 1);
    // Keeps Focus at half width until the other toggles arrive.
    grid.attach(&gtk::Box::new(gtk::Orientation::Horizontal, 0), 1, 0, 1, 1);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 14);
    card.add_css_class("card");
    card.add_css_class("quick");
    card.append(&grid);
    card
}
