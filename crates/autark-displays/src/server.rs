//! `org.autark.Displays1` on the session bus, for Settings › Display and Kith. Monitors go over the bus as
//! JSON: `List()` gives the heads (`layout::Head`), `Apply()` takes outputs (`layout::Output`) by id or connector.

use std::sync::{Arc, Mutex};

use zbus::object_server::SignalEmitter;

use crate::layout::Output;

pub const PATH: &str = "/org/autark/Displays1";

pub type Reply = async_channel::Sender<Result<(), String>>;

pub struct Displays {
    /// The heads as JSON, kept current by the daemon.
    pub heads: Arc<Mutex<String>>,
    pub requests: async_channel::Sender<(Vec<Output>, Reply)>,
}

#[zbus::interface(name = "org.autark.Displays1")]
impl Displays {
    /// Every connected monitor with its modes and how it's set now.
    async fn list(&self) -> String {
        self.heads.lock().unwrap().clone()
    }

    /// Sets monitors as given (others stay as they are), and remembers it for this set of monitors.
    async fn apply(&self, outputs: &str) -> zbus::fdo::Result<()> {
        let outputs: Vec<Output> = serde_json::from_str(outputs).map_err(|error| zbus::fdo::Error::InvalidArgs(error.to_string()))?;
        let (reply, answer) = async_channel::bounded(1);
        self.requests.send((outputs, reply)).await.map_err(|_| zbus::fdo::Error::Failed("the daemon is stopping".into()))?;
        answer.recv().await.map_err(|_| zbus::fdo::Error::Failed("no answer".into()))?.map_err(zbus::fdo::Error::Failed)
    }

    /// The heads changed: plugged in, unplugged, or set differently.
    #[zbus(signal)]
    pub async fn changed(emitter: &SignalEmitter<'_>, heads: &str) -> zbus::Result<()>;
}
