//! Power from the root helper (`autark-stats-power`), for when the RAPL counter isn't readable
//! by us directly, as on any kernel since 2020.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[zbus::proxy(interface = "org.autark.StatsPower1", default_service = "org.autark.StatsPower1", default_path = "/org/autark/StatsPower1")]
trait StatsPower {
    fn watts(&self) -> zbus::Result<f64>;
}

#[derive(Default)]
pub struct Helper {
    connection: Mutex<Option<zbus::Connection>>,
    /// After a failure (helper not installed, no RAPL), when to try again.
    retry_at: Mutex<Option<Instant>>,
}

impl Helper {
    pub async fn watts(&self) -> Option<f64> {
        if crate::sample::rapl_readable() || self.retry_at.lock().unwrap().is_some_and(|at| Instant::now() < at) {
            return None;
        }
        let result = async {
            let existing = self.connection.lock().unwrap().clone();
            let connection = match existing {
                Some(connection) => connection,
                None => {
                    let connection = zbus::Connection::system().await?;
                    *self.connection.lock().unwrap() = Some(connection.clone());
                    connection
                }
            };
            StatsPowerProxy::new(&connection).await?.watts().await
        };
        match result.await {
            Ok(watts) => Some(watts),
            Err(err) => {
                eprintln!("stats: power helper: {err}; trying again in 5 minutes");
                *self.retry_at.lock().unwrap() = Some(Instant::now() + Duration::from_secs(300));
                None
            }
        }
    }
}
