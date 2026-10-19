//! Kith's `Check` for pastes: a paste is what hands a shell a whole command it never saw you type. The daemon rates a
//! command's risk from 0 to 1, with built-in rules for destructive ones that work even with AI off. Only a paste that
//! runs on its own (it has a newline) is rated, and only a high rating stops it for a confirmation; if the daemon can't
//! be reached, the paste goes through as it would in any terminal.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// At or above this rating the paste waits for a yes.
pub const ASK_AT: f64 = 0.6;

pub struct Rating {
    pub risk: f64,
    pub reason: String,
}

/// Whether `text` would run by itself when pasted: it has a line break with something after the first line's start.
pub fn runs_itself(text: &str) -> bool {
    text.trim_end_matches(['\n', '\r']).contains('\n') || text.ends_with('\n')
}

/// Asks `org.autark.AI1` to rate `command`; `done` gets `None` if the daemon can't be reached in time.
pub fn rate(command: &str, done: impl FnOnce(Option<Rating>) + 'static) {
    let command = command.to_owned();
    glib::spawn_future_local(async move {
        done(rate_now(&command).await);
    });
}

async fn rate_now(command: &str) -> Option<Rating> {
    let bus = gio::bus_get_future(gio::BusType::Session).await.ok()?;
    let reply = bus
        .call_future(
            Some("org.autark.AI1"),
            "/org/autark/AI1",
            "org.autark.AI1",
            "Check",
            Some(&(command,).to_variant()),
            Some(glib::VariantTy::new("(ds)").ok()?),
            gio::DBusCallFlags::NONE,
            4000,
        )
        .await
        .ok()?;
    let (risk, reason): (f64, String) = reply.get()?;
    Some(Rating { risk, reason })
}
