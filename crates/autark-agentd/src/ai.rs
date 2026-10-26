//! `autark-ai`'s two calls for the team, over the session bus: the model (`Complete`) and what an agent may look at
//! (`Look`). Kith's own conversation is read and added to through it too.

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
pub trait Ai {
    fn complete(&self, system: &str, context: &str, prompt: &str) -> zbus::Result<u32>;
    fn look(&self, question: &str, can: &[&str]) -> zbus::Result<String>;
    fn stop(&self, id: u32) -> zbus::Result<()>;
    fn history(&self) -> zbus::Result<String>;
    fn clear_history(&self) -> zbus::Result<()>;
    fn chat(&self, text: &str) -> zbus::Result<String>;
    fn chat_answer(&self, question: &str, files: &[&str]) -> zbus::Result<(u32, u64, Vec<(String, String)>)>;
    #[zbus(signal)]
    fn answer_piece(&self, id: u32, offset: u32, text: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn answer_done(&self, id: u32, error: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn history_changed(&self) -> zbus::Result<()>;
}

/// What a failed call says, without D-Bus's prefix.
pub fn said(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(detail), _) => detail.clone(),
        zbus::Error::MethodError(..) | zbus::Error::InputOutput(_) => "Kith isn't running. Start autark-ai and try again.".to_owned(),
        other => other.to_string(),
    }
}
