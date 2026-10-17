//! Notification centre and org.freedesktop.Notifications service, with widgets.

mod card;
mod centre;
mod config;
mod server;
mod shared;
mod widgets;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use zbus::object_server::SignalEmitter;

use server::{Event, Notification, Server};

pub const WIDTH: i32 = 380;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Notifyd").build();
    app.connect_startup(|_| load_css());
    app.connect_activate(start);
    app.run()
}

fn start(app: &gtk::Application) {
    thread_local!(static STARTED: Cell<bool> = const { Cell::new(false) });
    if STARTED.replace(true) {
        return;
    }
    // Runs with no window open, between notifications.
    std::mem::forget(app.hold());

    let (events, received) = async_channel::unbounded();
    let kept = Arc::new(Mutex::new(Vec::new()));
    let app = app.clone();
    glib::spawn_future_local(async move {
        let connection = zbus::connection::Builder::session()
            .name("org.freedesktop.Notifications")
            .name("org.autark.Notifications1")
            .serve_at(server::PATH, Server::new(events.clone()))
            .serve_at(server::CONTROL_PATH, server::Control { events, kept: kept.clone() })
            .build()
            .await;
        let connection = match connection {
            Ok(connection) => connection,
            Err(err) => {
                eprintln!("notifyd: can't serve notifications (is another notification daemon running?): {err}");
                app.quit();
                return;
            }
        };
        let daemon = Daemon::new(&app, connection, kept);
        while let Ok(event) = received.recv().await {
            match event {
                Event::Notify(notification) => daemon.notify(notification),
                Event::Close(id) => daemon.dismiss(id, server::CLOSED_BY_APP),
                Event::Toggle => daemon.toggle_centre(),
                Event::EditWidgets => daemon.centre.edit_widgets(),
                Event::Focus(on) => daemon.centre.set_focus(on),
            }
        }
    });
}

pub struct Daemon {
    connection: zbus::Connection,
    config: config::Notifyd,
    /// Kept for the centre, newest first.
    notifications: RefCell<Vec<Notification>>,
    /// The same, as `org.autark.Notifications1.Kept` gives them to the lock screen.
    kept: Arc<Mutex<Vec<server::Kept>>>,
    pub do_not_disturb: Cell<bool>,
    banner_window: gtk::ApplicationWindow,
    banners: gtk::Box,
    centre: centre::Centre,
    handlers: Rc<card::Handlers>,
    _desktop: gtk::ApplicationWindow,
    /// Widgets hold only weak references to their sources.
    _sources: Rc<widgets::Sources>,
}

impl Daemon {
    fn new(app: &gtk::Application, connection: zbus::Connection, kept: Arc<Mutex<Vec<server::Kept>>>) -> Rc<Self> {
        Rc::new_cyclic(|daemon: &Weak<Self>| {
            let banners = gtk::Box::new(gtk::Orientation::Vertical, 8);
            let banner_window = gtk::ApplicationWindow::new(app);
            banner_window.init_layer_shell();
            banner_window.set_layer(Layer::Overlay);
            banner_window.set_namespace(Some("autark-notifications"));
            banner_window.set_anchor(Edge::Top, true);
            banner_window.set_anchor(Edge::Right, true);
            banner_window.set_margin(Edge::Top, 12);
            banner_window.set_margin(Edge::Right, 12);
            banner_window.set_default_size(WIDTH, -1);
            banner_window.add_css_class("notifications");
            banner_window.set_child(Some(&banners));

            let handlers = Rc::new(card::Handlers {
                action: Box::new({
                    let daemon = daemon.clone();
                    move |id, key| {
                        if let Some(daemon) = daemon.upgrade() {
                            daemon.invoke(id, key);
                        }
                    }
                }),
                dismiss: Box::new({
                    let daemon = daemon.clone();
                    move |id| {
                        if let Some(daemon) = daemon.upgrade() {
                            daemon.dismiss(id, server::DISMISSED);
                        }
                    }
                }),
            });
            let config: config::Notifyd = autark_common::config::load("notifyd");
            let sources = widgets::Sources::new(&config);
            Self {
                connection,
                config,
                notifications: RefCell::default(),
                kept,
                do_not_disturb: Cell::new(false),
                banner_window,
                banners,
                centre: centre::Centre::new(app, daemon.clone(), &sources),
                handlers,
                _desktop: widgets::desktop(app, &sources),
                _sources: sources,
            }
        })
    }

    pub fn notifications(&self) -> Vec<Notification> {
        self.notifications.borrow().clone()
    }

    pub fn handlers(&self) -> Rc<card::Handlers> {
        self.handlers.clone()
    }

    fn notify(self: &Rc<Self>, notification: Notification) {
        let update;
        {
            let mut kept = self.notifications.borrow_mut();
            update = kept.iter().any(|kept| kept.id == notification.id) || self.banner(notification.id).is_some();
            kept.retain(|kept| kept.id != notification.id);
            if !notification.transient {
                kept.insert(0, notification.clone());
            }
        }
        self.share_kept();
        // With the centre open on them, new notifications show up there instead of as banners.
        let quiet = self.do_not_disturb.get() && notification.urgency != server::CRITICAL;
        if !quiet && !self.centre.shows_notifications() {
            self.show_banner(&notification);
        }
        // A chime for something new; an update to one already shown (progress, say) comes quietly.
        if !quiet && !update && !notification.silent && notification.urgency != server::LOW {
            autark_common::audio::play(autark_common::audio::Effect::Alert);
        }
        self.centre.refresh();
    }

    fn show_banner(self: &Rc<Self>, notification: &Notification) {
        self.remove_banner(notification.id);
        let card = card::new(notification, &self.handlers);
        card.set_widget_name(&notification.id.to_string());
        self.banners.prepend(&card);
        while self.banner_count() > self.config.max_banners {
            if let Some(oldest) = self.banners.last_child() {
                self.banners.remove(&oldest);
            }
        }
        self.banner_window.set_visible(true);

        let sticky = notification.timeout == 0 || notification.urgency == server::CRITICAL;
        if sticky {
            return;
        }
        let milliseconds = match notification.timeout {
            timeout if timeout > 0 => timeout as u64,
            _ => u64::from(self.config.banner_seconds) * 1000,
        };
        let (daemon, card, id, transient) = (Rc::downgrade(self), card.downgrade(), notification.id, notification.transient);
        glib::timeout_add_local_once(Duration::from_millis(milliseconds), move || {
            let Some(daemon) = daemon.upgrade() else {
                return;
            };
            // Only this card: a replacement under the same ID has its own timer.
            if let Some(card) = card.upgrade().filter(|card| card.parent().is_some()) {
                daemon.banners.remove(&card);
                daemon.hide_if_empty();
                if transient {
                    daemon.emit_closed(id, server::EXPIRED);
                }
            }
        });
    }

    fn banner_count(&self) -> usize {
        std::iter::successors(self.banners.first_child(), |child| child.next_sibling()).count()
    }

    fn banner(&self, id: u32) -> Option<gtk::Widget> {
        let name = id.to_string();
        std::iter::successors(self.banners.first_child(), |child| child.next_sibling()).find(|child| child.widget_name() == name)
    }

    fn remove_banner(&self, id: u32) {
        if let Some(card) = self.banner(id) {
            self.banners.remove(&card);
        }
        self.hide_if_empty();
    }

    fn hide_if_empty(&self) {
        if self.banners.first_child().is_none() {
            self.banner_window.set_visible(false);
        }
    }

    /// Banners make way for the centre: everything they show is in it.
    fn toggle_centre(&self) {
        self.centre.toggle();
        if self.centre.shows_notifications() {
            while let Some(card) = self.banners.first_child() {
                self.banners.remove(&card);
            }
            self.hide_if_empty();
        }
    }

    pub fn dismiss(&self, id: u32, reason: u32) {
        self.notifications.borrow_mut().retain(|kept| kept.id != id);
        self.share_kept();
        self.remove_banner(id);
        self.centre.refresh();
        self.emit_closed(id, reason);
    }

    pub fn clear(&self) {
        let ids: Vec<u32> = self.notifications.borrow().iter().map(|kept| kept.id).collect();
        for id in ids {
            self.dismiss(id, server::DISMISSED);
        }
    }

    /// Copies the kept notifications to where `Kept` answers from, and says they changed.
    fn share_kept(&self) {
        if let Ok(mut kept) = self.kept.lock() {
            *kept = self.notifications.borrow().iter().map(Notification::kept).collect();
        }
        let connection = self.connection.clone();
        glib::spawn_future_local(async move {
            if let Ok(emitter) = SignalEmitter::new(&connection, server::CONTROL_PATH) {
                let _ = server::Control::kept_changed(&emitter).await;
            }
        });
    }

    fn invoke(&self, id: u32, key: &str) {
        let (connection, key) = (self.connection.clone(), key.to_owned());
        glib::spawn_future_local(async move {
            if let Ok(emitter) = SignalEmitter::new(&connection, server::PATH) {
                let _ = Server::action_invoked(&emitter, id, &key).await;
            }
        });
    }

    fn emit_closed(&self, id: u32, reason: u32) {
        let connection = self.connection.clone();
        glib::spawn_future_local(async move {
            if let Ok(emitter) = SignalEmitter::new(&connection, server::PATH) {
                let _ = Server::notification_closed(&emitter, id, reason).await;
            }
        });
    }
}

fn load_css() {
    autark_common::theme::install(include_str!("notifyd.css"));
}
