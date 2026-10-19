//! `autark-ai`'s calls as the launcher and the Assistant panel use them, and what they share in showing Kith's
//! actions and errors.

use std::cell::RefCell;

use serde_json::Value;

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
pub trait Ai {
    fn chat(&self, text: &str) -> zbus::Result<String>;
    fn chat_answer(&self, question: &str, files: &[&str]) -> zbus::Result<(u32, u64, Vec<(String, String)>)>;
    fn chat_act(&self, entry: u64, chosen: &[u32]) -> zbus::Result<Vec<String>>;
    fn remember(&self, entry: u64, text: &str, save: bool) -> zbus::Result<()>;
    fn create_task(&self, entry: u64, task: &str) -> zbus::Result<(String, u64)>;
    fn cancel_task(&self, entry: u64) -> zbus::Result<()>;
    fn answer_suggestion(&self, entry: u64, answer: &str) -> zbus::Result<String>;
    fn history(&self) -> zbus::Result<String>;
    fn clear_history(&self) -> zbus::Result<()>;
    fn stop(&self, id: u32) -> zbus::Result<()>;
    #[zbus(property(emits_changed_signal = "false"))]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(signal)]
    fn state_changed(&self, state: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn answer_piece(&self, id: u32, offset: u32, text: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn answer_done(&self, id: u32, error: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn history_changed(&self) -> zbus::Result<()>;
}

thread_local! {
    static SESSION: RefCell<Option<zbus::Connection>> = const { RefCell::new(None) };
}

/// `autark-ai` on the session bus, over one connection for the whole launcher.
pub async fn proxy() -> zbus::Result<AiProxy<'static>> {
    let cached = SESSION.with(|session| session.borrow().clone());
    let connection = match cached {
        Some(connection) => connection,
        None => {
            let connection = zbus::Connection::session().await?;
            SESSION.with(|session| *session.borrow_mut() = Some(connection.clone()));
            connection
        }
    };
    AiProxy::new(&connection).await
}

/// An action's icon, title, new value and what runs it, from `Act`'s catalog.
pub fn describe(action: &Value) -> (&'static str, String, String, String) {
    let on = action["on"].as_bool().unwrap_or(false);
    let percent = action["percent"].as_u64().unwrap_or(0);
    let state = if on { "on" } else { "off" }.to_owned();
    let (icon, title, value, call) = match action["action"].as_str().unwrap_or_default() {
        "set_volume" => ("audio-volume-high-symbolic", "Volume", format!("{percent}%"), format!("wpctl set-volume @DEFAULT_AUDIO_SINK@ {percent}%")),
        "mute" => (
            if on { "audio-volume-muted-symbolic" } else { "audio-volume-high-symbolic" },
            "Volume",
            if on { "muted" } else { "unmuted" }.to_owned(),
            format!("wpctl set-mute @DEFAULT_AUDIO_SINK@ {}", u8::from(on)),
        ),
        "set_brightness" => ("display-brightness-symbolic", "Display brightness", format!("{percent}%"), format!("login1.Session.SetBrightness(backlight, {percent}%)")),
        "wifi" => ("network-wireless-symbolic", "Wi-Fi", state, format!("iwd Device.Powered = {on}")),
        "focus" => ("notifications-disabled-symbolic", "Focus", state, format!("Notifications1.SetFocus({on})")),
        "night_light" => ("night-light-symbolic", "Night light", state, format!("NightLight1.SetActive({on})")),
        "lock" => ("system-lock-screen-symbolic", "Lock screen", String::new(), "login1.Session.Lock()".to_owned()),
        "sleep" => ("weather-clear-night-symbolic", "Sleep", String::new(), "login1.Manager.Suspend(true)".to_owned()),
        "open_app" => {
            let name = action["name"].as_str().unwrap_or_default().to_owned();
            ("application-x-executable-symbolic", "Open", name.clone(), format!("open the app called “{name}”"))
        }
        "open_url" => {
            let url = action["url"].as_str().unwrap_or_default().to_owned();
            ("web-browser-symbolic", "Open website", url.clone(), format!("open {url} in the browser"))
        }
        other => ("dialog-question-symbolic", other, String::new(), String::new()),
    };
    (icon, title.to_owned(), value, call)
}

/// A D-Bus error in words: autark-ai's own errors as it wrote them.
pub fn said(error: zbus::Error) -> String {
    match &error {
        zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" || name.as_str().starts_with("org.freedesktop.DBus.Error.Spawn.") => {
            "Kith isn’t running: start autark-ai.".to_owned()
        }
        zbus::Error::FDO(inner) if matches!(**inner, zbus::fdo::Error::ServiceUnknown(_)) => "Kith isn’t running: start autark-ai.".to_owned(),
        zbus::Error::MethodError(_, Some(message), _) => message.clone(),
        _ => error.to_string(),
    }
}

/// Adds a piece of an answer that starts at byte `offset` of it, to `text` holding what's come so far: a piece
/// already in it is skipped, so text read from `History` and the pieces arriving meanwhile join up.
pub fn merge(text: &mut String, offset: u32, piece: &str) {
    let offset = offset as usize;
    let have = text.len();
    if offset >= have {
        text.push_str(piece);
    } else if offset + piece.len() > have
        && let Some(rest) = piece.get(have - offset..)
    {
        text.push_str(rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_join_what_was_read() {
        let mut text = "Your last".to_owned();
        merge(&mut text, 0, "Your");
        merge(&mut text, 4, " last");
        merge(&mut text, 9, " build");
        assert_eq!(text, "Your last build");
        let mut text = "Your la".to_owned();
        merge(&mut text, 4, " last");
        assert_eq!(text, "Your last");
    }
}
