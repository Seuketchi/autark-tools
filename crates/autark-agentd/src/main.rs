//! The team's backend (`org.autark.Agents1`): Kith's agents, their conversations, memory and the event log. See
//! `docs/agentd.md`. It never reads your mail or talks to a model itself: `autark-ai` does, under the rules it has.

mod agents;
mod ai;
mod chat;
mod events;
mod files;
mod memory;
mod notes;
mod prompt;
mod remember;
mod service;

use std::sync::Arc;
use std::time::Duration;

/// How long a call may wait for an answer, to `autark-ai` and from a client.
const CALLS: Duration = Duration::from_secs(10 * 60);

fn main() {
    agents::seed();
    agents::seed_lenses();
    let shared = Arc::new(service::Shared::new(events::Events::new(agents::state_base().join("events.jsonl"))));
    let result = async_io::block_on(async {
        // A call to Kith can wait on a model that is loading or a busy laptop for longer than D-Bus's 25 seconds.
        let connection = zbus::connection::Builder::session().method_timeout(CALLS).name("org.autark.Agents1").serve_at(service::PATH, service::Agents { shared: shared.clone() }).build().await?;
        // Only once the name is ours, so a second daemon that won't get it never rewrites the log under the first.
        shared.prune();
        // Files an earlier version wrote 0644 are narrowed once, not only when each is written again.
        agents::tighten();
        service::follow(shared, connection).await;
        Ok::<(), zbus::Error>(())
    });
    if let Err(err) = result {
        eprintln!("agentd: {err}");
        std::process::exit(1);
    }
}
