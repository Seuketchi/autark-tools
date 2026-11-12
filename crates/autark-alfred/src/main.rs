//! The team's backend (`org.autark.Alfred1`): Kith's agents, their conversations, memory and the event log. See
//! `docs/alfred.md`. It never reads your mail or talks to a model itself: `autark-ai` does, under the rules it has.

mod agents;
mod ai;
mod chat;
mod events;
mod files;
mod jobs;
mod mailbox;
mod memory;
mod moved;
mod notes;
mod notify;
mod prompt;
mod remember;
mod service;
mod when;

use std::sync::Arc;
use std::time::Duration;

/// How long a call may wait for an answer, to `autark-ai` and from a client.
const CALLS: Duration = Duration::from_secs(10 * 60);

fn main() {
    // The log's path is where it will be once the old folders are moved below. Until then it reads as empty, which only
    // means the next event's number comes from the clock, larger than any before it.
    let shared = Arc::new(service::Shared::new(events::Events::new(agents::state_base().join("events.jsonl"))));
    let result = async_io::block_on(async {
        // A call to Kith can wait on a model that is loading or a busy laptop for longer than D-Bus's 25 seconds.
        let connection = zbus::connection::Builder::session().method_timeout(CALLS).name("org.autark.Alfred1").serve_at(service::PATH, service::Agents { shared: shared.clone() }).build().await?;
        // Only once the name is ours, so a second daemon that won't get it never moves the folders or rewrites the log
        // under the first: the old name's folders are moved before the starter team is made in the new ones.
        moved::migrate();
        agents::seed();
        agents::seed_lenses();
        agents::seed_links();
        // A client that asked for the team while it was being made asks again.
        let emitter = zbus::object_server::SignalEmitter::new(&connection, service::PATH)?;
        if let Err(err) = service::Agents::agents_changed(&emitter).await {
            eprintln!("alfred: sending a change: {err}");
        }
        shared.prune();
        // Files an earlier version wrote 0644 are narrowed once, not only when each is written again.
        agents::tighten();
        service::follow(shared, connection).await;
        Ok::<(), zbus::Error>(())
    });
    if let Err(err) = result {
        eprintln!("alfred: {err}");
        std::process::exit(1);
    }
}
