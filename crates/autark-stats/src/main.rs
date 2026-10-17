//! System metrics over D-Bus (`org.autark.Stats1`): CPU, memory, disk, network, battery health,
//! and each app's share of them from its cgroup, with an energy estimate. It reads only while
//! something is watching, so it costs nothing when no one looks.

mod power;
mod sample;
mod server;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use zbus::object_server::SignalEmitter;

use sample::Sampler;
use server::Stats;

fn main() {
    if let Err(err) = async_io::block_on(run()) {
        eprintln!("stats: {err}");
        std::process::exit(1);
    }
}

async fn run() -> zbus::Result<()> {
    let (wake, woken) = async_channel::unbounded();
    let rewake = wake.clone();
    let stats = Stats {
        watchers: Arc::default(),
        sampler: Arc::new(Mutex::new(Sampler::new())),
        latest: Arc::default(),
        helper: Arc::default(),
        wake,
    };
    let (watchers, sampler, latest, helper) = (stats.watchers.clone(), stats.sampler.clone(), stats.latest.clone(), stats.helper.clone());
    let connection = zbus::connection::Builder::session().name("org.autark.Stats1").serve_at(server::PATH, stats).build().await?;

    // A watcher that leaves the bus stops counting.
    let departures = {
        let (connection, watchers) = (connection.clone(), watchers.clone());
        async move {
            let mut changes = zbus::fdo::DBusProxy::new(&connection).await?.receive_name_owner_changed().await?;
            while let Some(change) = changes.next().await {
                let Ok(args) = change.args() else { continue };
                if args.new_owner().is_none() && watchers.lock().unwrap().remove(args.name().as_str()).is_some() {
                    let _ = rewake.try_send(());
                }
            }
            zbus::Result::Ok(())
        }
    };

    let polling = async move {
        let emitter = SignalEmitter::new(&connection, server::PATH)?;
        loop {
            let interval = watchers.lock().unwrap().values().min().copied();
            let Some(interval) = interval else {
                // Nobody is looking: sleep until someone calls Watch.
                let _ = woken.recv().await;
                continue;
            };
            let watts = helper.watts().await;
            let sampler = sampler.clone();
            let snapshot = blocking::unblock(move || sampler.lock().unwrap().sample(watts)).await;
            let json = serde_json::to_string(&snapshot).unwrap_or_default();
            *latest.lock().unwrap() = json.clone();
            if let Err(err) = Stats::updated(&emitter, &json).await {
                eprintln!("stats: sending an update: {err}");
            }
            // Until the next reading, or sooner if the watchers change.
            let tick = std::pin::pin!(async_io::Timer::after(Duration::from_millis(interval.into())));
            let change = std::pin::pin!(woken.recv());
            futures_util::future::select(tick, change).await;
        }
        #[allow(unreachable_code)]
        zbus::Result::Ok(())
    };

    futures_util::try_join!(departures, polling)?;
    Ok(())
}
