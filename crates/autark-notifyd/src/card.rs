//! A notification card, the same in banners and the centre: icon, app and time, summary, body,
//! and action buttons.

use std::rc::Rc;
use std::time::SystemTime;

use gtk::glib;
use gtk::prelude::*;

use crate::server::Notification;

type ActionHandler = Box<dyn Fn(u32, &str)>;

/// What a card asks for: invoke an action key on a notification, or dismiss it.
pub struct Handlers {
    pub action: ActionHandler,
    pub dismiss: Box<dyn Fn(u32)>,
}

pub fn new(notification: &Notification, handlers: &Rc<Handlers>) -> gtk::Box {
    let id = notification.id;

    let app = gtk::Label::builder()
        .label(&notification.app_name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    app.add_css_class("meta");
    let when = gtk::Label::new(Some(&ago(notification.received)));
    when.add_css_class("meta");
    let close = gtk::Button::with_label("×");
    close.add_css_class("close");
    close.set_tooltip_text(Some("Dismiss"));
    close.connect_clicked({
        let handlers = handlers.clone();
        move |_| (handlers.dismiss)(id)
    });
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&app);
    top.append(&when);
    top.append(&close);

    let summary = gtk::Label::builder()
        .label(&notification.summary)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    summary.add_css_class("summary");

    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    text.append(&top);
    text.append(&summary);
    if !notification.body.is_empty() {
        let body = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        // The spec allows simple markup; anything Pango can't parse is shown as plain text.
        if gtk::pango::parse_markup(&notification.body, '\0').is_ok() {
            body.set_markup(&notification.body);
        } else {
            body.set_text(&notification.body);
        }
        body.add_css_class("body");
        text.append(&body);
    }
    let buttons: Vec<_> = notification.actions.iter().filter(|(key, _)| key != "default").collect();
    if !buttons.is_empty() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("actions");
        for (key, label) in buttons {
            let button = gtk::Button::with_label(label);
            button.add_css_class("action");
            button.set_hexpand(true);
            let (handlers, key) = (handlers.clone(), key.clone());
            button.connect_clicked(move |_| (handlers.action)(id, &key));
            row.append(&button);
        }
        text.append(&row);
    }

    let card = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    card.add_css_class("card");
    if notification.urgency == crate::server::CRITICAL {
        card.add_css_class("critical");
    }
    card.append(&icon(notification));
    card.append(&text);

    // Clicking the card runs its default action if it has one, then dismisses it.
    let has_default = notification.actions.iter().any(|(key, _)| key == "default");
    let click = gtk::GestureClick::new();
    click.connect_released({
        let handlers = handlers.clone();
        move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if has_default {
                (handlers.action)(id, "default");
            }
            (handlers.dismiss)(id);
        }
    });
    card.add_controller(click);
    card
}

/// The app's icon from a path or the icon theme, else a tile with the app's initial.
fn icon(notification: &Notification) -> gtk::Widget {
    let name = notification.icon.strip_prefix("file://").unwrap_or(&notification.icon);
    let image = if name.starts_with('/') {
        Some(gtk::Image::from_file(name))
    } else if !name.is_empty() && gtk::IconTheme::for_display(&gtk::gdk::Display::default().unwrap()).has_icon(name) {
        Some(gtk::Image::from_icon_name(name))
    } else {
        None
    };
    if let Some(image) = image {
        image.set_pixel_size(36);
        image.set_valign(gtk::Align::Start);
        image.add_css_class("app-icon");
        return image.upcast();
    }
    let initial = notification.app_name.chars().next().unwrap_or('•').to_uppercase().to_string();
    let tile = gtk::Label::new(Some(&initial));
    tile.add_css_class("tile");
    tile.set_valign(gtk::Align::Start);
    tile.upcast()
}

/// "now", "12 min ago", "1 h ago", then the date.
fn ago(received: SystemTime) -> String {
    let minutes = received.elapsed().map(|elapsed| elapsed.as_secs() / 60).unwrap_or(0);
    match minutes {
        0 => "now".to_owned(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => {
            let seconds = received.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            glib::DateTime::from_unix_local(seconds as i64)
                .and_then(|date| date.format("%-d %b"))
                .map(|date| date.to_string())
                .unwrap_or_default()
        }
    }
}
