//! The feed from `autark-stats`: one `Snapshot` a second while the window is open, and the
//! last ten minutes kept for the charts.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use autark_common::stats::Snapshot;
use futures_util::StreamExt;
use gtk::glib;

#[zbus::proxy(interface = "org.autark.Stats1", default_service = "org.autark.Stats1", default_path = "/org/autark/Stats1")]
trait Stats {
    fn watch(&self, interval_ms: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn updated(&self, snapshot: &str) -> zbus::Result<()>;
}

pub enum Event {
    Snapshot(Box<Snapshot>),
    /// The service isn't there; the text says why.
    Unavailable(String),
}

/// Calls `on_event` with every update for as long as the main loop runs, reconnecting if
/// `autark-stats` restarts.
pub fn follow(on_event: impl Fn(Event) + 'static) {
    glib::spawn_future_local(async move {
        loop {
            let result = async {
                let connection = zbus::Connection::session().await?;
                let proxy = StatsProxy::new(&connection).await?;
                let mut updates = proxy.receive_updated().await?;
                proxy.watch(1000).await?;
                while let Some(update) = updates.next().await {
                    match serde_json::from_str::<Snapshot>(update.args()?.snapshot) {
                        Ok(snapshot) => on_event(Event::Snapshot(Box::new(snapshot))),
                        Err(err) => eprintln!("monitor: an update didn't parse: {err}"),
                    }
                }
                zbus::Result::Ok(())
            };
            if let Err(err) = result.await {
                on_event(Event::Unavailable(format!("autark-stats isn't running ({err})")));
            }
            glib::timeout_future(Duration::from_secs(5)).await;
        }
    });
}

#[derive(Clone, Copy, Default)]
pub struct Sample {
    pub cpu: f64,
    pub memory: f64,
    pub watts: Option<f64>,
    pub disk_read: f64,
    pub disk_write: f64,
    pub received: f64,
    pub sent: f64,
}

/// Ten minutes of readings, and how long each app has been busy.
#[derive(Default)]
pub struct History {
    pub samples: VecDeque<Sample>,
    busy_since: HashMap<String, Instant>,
}

const KEPT: usize = 600;
/// An app above this, in percent of a core, counts as busy.
const BUSY: f64 = 80.0;
/// How long an app stays busy before it's flagged.
const FLAG_AFTER: Duration = Duration::from_secs(120);

impl History {
    pub fn add(&mut self, snapshot: &Snapshot) {
        let memory = if snapshot.memory.total > 0 { snapshot.memory.used as f64 * 100.0 / snapshot.memory.total as f64 } else { 0.0 };
        self.samples.push_back(Sample {
            cpu: snapshot.cpu.total,
            memory,
            watts: snapshot.power.as_ref().map(|power| power.watts),
            disk_read: snapshot.disk.read as f64,
            disk_write: snapshot.disk.write as f64,
            received: snapshot.network.read as f64,
            sent: snapshot.network.write as f64,
        });
        while self.samples.len() > KEPT {
            self.samples.pop_front();
        }
        let now = Instant::now();
        self.busy_since.retain(|id, _| snapshot.apps.iter().any(|app| &app.id == id && app.cpu >= BUSY));
        for app in snapshot.apps.iter().filter(|app| app.cpu >= BUSY) {
            self.busy_since.entry(app.id.clone()).or_insert(now);
        }
    }

    /// "High CPU for 14 min" for an app that has stayed busy.
    pub fn flag(&self, id: &str) -> Option<String> {
        let since = self.busy_since.get(id)?.elapsed();
        (since >= FLAG_AFTER).then(|| format!("High CPU for {} min", since.as_secs() / 60))
    }
}
