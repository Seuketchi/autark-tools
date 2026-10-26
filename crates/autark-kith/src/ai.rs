//! `autark-ai`'s calls for the team, over one connection for the whole app.

use std::cell::RefCell;

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
pub trait Ai {
    fn agents(&self) -> zbus::Result<String>;
    fn create_agent(&self, name: &str, job: &str, soul: &str, can: &[&str]) -> zbus::Result<String>;
    fn delete_agent(&self, id: &str) -> zbus::Result<()>;
    fn agent_history(&self, id: &str) -> zbus::Result<String>;
    fn agent_say(&self, id: &str, text: &str) -> zbus::Result<(u32, u64)>;
    fn clear_agent(&self, id: &str) -> zbus::Result<()>;
    fn stop(&self, id: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn answer_piece(&self, id: u32, offset: u32, text: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn agent_changed(&self, id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn agents_changed(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn history_changed(&self) -> zbus::Result<()>;
}

thread_local! {
    static SESSION: RefCell<Option<zbus::Connection>> = const { RefCell::new(None) };
}

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

/// What a failed call says, without D-Bus's prefix.
pub fn said(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(detail), _) => detail.clone(),
        zbus::Error::MethodError(..) | zbus::Error::InputOutput(_) => "Kith isn't running. Start autark-ai and try again.".to_owned(),
        other => other.to_string(),
    }
}
