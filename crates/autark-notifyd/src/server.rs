//! The `org.freedesktop.Notifications` service apps send to, and `org.autark.Notifications1`,
//! which the bar calls to open the centre. Methods run on zbus's thread and hand events to GTK.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use async_channel::Sender;
use zbus::OwnedVariant;
use zbus::object_server::SignalEmitter;

pub const PATH: &str = "/org/freedesktop/Notifications";
pub const CONTROL_PATH: &str = "/org/autark/Notifications";

/// The spec's reasons for `NotificationClosed`.
pub const EXPIRED: u32 = 1;
pub const DISMISSED: u32 = 2;
pub const CLOSED_BY_APP: u32 = 3;

pub const CRITICAL: u8 = 2;

#[derive(Clone)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    /// An icon name or a file path.
    pub icon: String,
    pub summary: String,
    pub body: String,
    /// Pairs of action key and label; the key "default" is invoked by clicking the notification.
    pub actions: Vec<(String, String)>,
    pub urgency: u8,
    /// In milliseconds: -1 leaves it to us, 0 never expires.
    pub timeout: i32,
    /// Shown as a banner only, never kept in the centre.
    pub transient: bool,
    pub received: SystemTime,
}

pub enum Event {
    Notify(Notification),
    Close(u32),
    Toggle,
}

pub struct Server {
    next_id: AtomicU32,
    events: Sender<Event>,
}

impl Server {
    pub fn new(events: Sender<Event>) -> Self {
        Self { next_id: AtomicU32::new(1), events }
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["actions", "body", "body-markup", "body-hyperlinks", "icon-static", "persistence"]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        expire_timeout: i32,
    ) -> u32 {
        let id = match replaces_id {
            0 => self.next_id.fetch_add(1, Ordering::Relaxed),
            id => id,
        };
        let hint = |name: &str| hints.get(name).and_then(|value| value.try_clone().ok());
        let image = hint("image-path").and_then(|value| String::try_from(value).ok());
        let notification = Notification {
            id,
            app_name,
            icon: image.filter(|path| !path.is_empty()).unwrap_or(app_icon),
            summary,
            body,
            actions: actions.as_chunks::<2>().0.iter().map(|[key, label]| (key.clone(), label.clone())).collect(),
            urgency: hint("urgency").and_then(|value| u8::try_from(value).ok()).unwrap_or(1),
            timeout: expire_timeout,
            transient: hint("transient").and_then(|value| bool::try_from(value).ok()).unwrap_or(false),
            received: SystemTime::now(),
        };
        let _ = self.events.send(Event::Notify(notification)).await;
        id
    }

    async fn close_notification(&self, id: u32) {
        let _ = self.events.send(Event::Close(id)).await;
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("Autark", "Autark", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    pub async fn notification_closed(emitter: &SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn action_invoked(emitter: &SignalEmitter<'_>, id: u32, action_key: &str) -> zbus::Result<()>;
}

pub struct Control {
    pub events: Sender<Event>,
}

#[zbus::interface(name = "org.autark.Notifications1")]
impl Control {
    /// Opens the notification centre, or closes it if open.
    async fn toggle(&self) {
        let _ = self.events.send(Event::Toggle).await;
    }
}
