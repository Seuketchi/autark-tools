//! The notification centre: a panel down the right edge with two views, Notifications (quick
//! settings and every kept notification) and Widgets. Opened from the bar; Escape closes it.

use std::f64::consts::PI;
use std::rc::{Rc, Weak};

use gtk::cairo::Operator;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::widgets::{self, Sources};
use crate::{Daemon, WIDTH};

pub struct Centre {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    list: gtk::Box,
    daemon: Weak<Daemon>,
    /// The switch's Widgets button, and Edit widgets on that view.
    widgets: gtk::ToggleButton,
    edit: gtk::MenuButton,
    /// Focus, in the quick settings.
    focus: gtk::ToggleButton,
}

impl Centre {
    pub fn new(app: &gtk::Application, daemon: Weak<Daemon>, sources: &Rc<Sources>) -> Self {
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
        let (quick, focus) = quick_settings(daemon.clone());
        content.append(&quick);
        content.append(&header);
        content.append(&list);
        let scroller = |child: &gtk::Box| gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(child).build();
        let stack = gtk::Stack::new();
        stack.add_named(&scroller(&content), Some("notifications"));
        let (widgets_page, edit) = widgets::page(sources);
        stack.add_named(&scroller(&widgets_page), Some("widgets"));

        let switch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        switch.add_css_class("switch");
        switch.set_homogeneous(true);
        let mut first: Option<gtk::ToggleButton> = None;
        let mut widgets_button = None;
        for (name, label) in [("notifications", "Notifications"), ("widgets", "Widgets")] {
            let button = gtk::ToggleButton::with_label(label);
            button.set_group(first.as_ref());
            button.set_active(first.is_none());
            let stack = stack.downgrade();
            button.connect_toggled(move |button| {
                if button.is_active()
                    && let Some(stack) = stack.upgrade()
                {
                    stack.set_visible_child_name(name);
                }
            });
            switch.append(&button);
            if name == "widgets" {
                widgets_button = Some(button.clone());
            }
            first.get_or_insert(button);
        }

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.append(&switch);
        column.append(&stack);
        window.set_child(Some(&column));

        // Widgets read their sources only while you can see them.
        let showing = {
            let (window, stack, sources) = (window.downgrade(), stack.downgrade(), Rc::downgrade(sources));
            move || {
                let (Some(window), Some(stack), Some(sources)) = (window.upgrade(), stack.upgrade(), sources.upgrade()) else {
                    return;
                };
                sources.set_shown(window.is_visible() && stack.visible_child_name().as_deref() == Some("widgets"));
            }
        };
        let also = showing.clone();
        window.connect_visible_notify(move |_| also());
        stack.connect_visible_child_name_notify(move |_| showing());

        let widgets = widgets_button.expect("the switch has a Widgets button");
        Self { window, stack, list, daemon, widgets, edit, focus }
    }

    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    /// Open on the Notifications view, where new notifications show up without a banner.
    pub fn shows_notifications(&self) -> bool {
        self.is_open() && self.stack.visible_child_name().as_deref() == Some("notifications")
    }

    /// Opens on the Widgets view with Edit widgets showing, for Settings' Edit widgets button.
    /// Focus on or off from elsewhere (Kith), as if its toggle were pressed.
    pub fn set_focus(&self, on: bool) {
        self.focus.set_active(on);
    }

    pub fn edit_widgets(&self) {
        if !self.window.is_visible() {
            self.window.set_visible(true);
            self.refresh();
        }
        self.widgets.set_active(true);
        self.window.present();
        // A popover placed before the centre is laid out lands in the wrong place.
        let edit = self.edit.downgrade();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
            if let Some(edit) = edit.upgrade() {
                edit.popup();
            }
        });
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
fn quick_settings(daemon: Weak<Daemon>) -> (gtk::Box, gtk::ToggleButton) {
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
    (card, focus)
}
