//! The team's backend (`autark-agentd`), over one connection for the whole app.

use std::cell::RefCell;

#[zbus::proxy(interface = "org.autark.Agents1", default_service = "org.autark.Agents1", default_path = "/org/autark/Agents1")]
pub trait Agents {
    fn agents(&self) -> zbus::Result<String>;
    fn create_agent(&self, name: &str, job: &str, soul: &str, can: &[&str], skills: &[&str], links: &[&str]) -> zbus::Result<String>;
    fn delete_agent(&self, id: &str) -> zbus::Result<()>;
    fn set_agent(&self, id: &str, patch: &str) -> zbus::Result<()>;
    fn history(&self, id: &str) -> zbus::Result<String>;
    fn say(&self, id: &str, text: &str) -> zbus::Result<(u32, u64)>;
    fn clear(&self, id: &str) -> zbus::Result<()>;
    fn memory(&self, id: &str) -> zbus::Result<String>;
    fn remember(&self, id: &str, text: &str) -> zbus::Result<()>;
    fn forget(&self, id: &str, n: u32) -> zbus::Result<()>;
    fn answer_fact(&self, id: &str, entry: u64, save: bool) -> zbus::Result<()>;
    fn answer_add(&self, id: &str, entry: u64, add: bool) -> zbus::Result<()>;
    fn answer_job(&self, id: &str, entry: u64, schedule: bool) -> zbus::Result<()>;
    fn jobs(&self, id: &str) -> zbus::Result<String>;
    fn add_job(&self, id: &str, when: &str, prompt: &str) -> zbus::Result<()>;
    fn remove_job(&self, id: &str, n: u32) -> zbus::Result<()>;
    fn set_job_enabled(&self, id: &str, n: u32, on: bool) -> zbus::Result<()>;
    fn run_job(&self, id: &str, n: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn answer_piece(&self, answer: u32, offset: u32, text: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn agent_changed(&self, id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn agents_changed(&self) -> zbus::Result<()>;
}

thread_local! {
    static SESSION: RefCell<Option<zbus::Connection>> = const { RefCell::new(None) };
}

pub async fn proxy() -> zbus::Result<AgentsProxy<'static>> {
    let cached = SESSION.with(|session| session.borrow().clone());
    let connection = match cached {
        Some(connection) => connection,
        None => {
            // `Say` returns once the agent has started answering, which can take longer than D-Bus's 25 seconds.
            let connection = zbus::connection::Builder::session().method_timeout(std::time::Duration::from_secs(10 * 60)).build().await?;
            SESSION.with(|session| *session.borrow_mut() = Some(connection.clone()));
            connection
        }
    };
    AgentsProxy::new(&connection).await
}

/// What a failed call says, without D-Bus's prefix.
pub fn said(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(detail), _) => detail.clone(),
        zbus::Error::MethodError(..) | zbus::Error::InputOutput(_) => "The team's backend isn't running. Start autark-agentd and try again.".to_owned(),
        other => other.to_string(),
    }
}
