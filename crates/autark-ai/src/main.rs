//! Kith's local daemon (org.autark.AI1), a minimal laptop supervisor.
//!
//! `Route` turns what's typed into laptop actions and a question, `Act` runs the chosen actions, `Ask` streams one
//! answer to a quick question about the laptop, `Check` rates a shell command's risk and `Urgent` tells Focus whether
//! a notification can't wait; `Models` lists Ollama's models. Suggestions come from what code notices (`events`),
//! rated by Ollama (`suggest`). Jev answers `Check`, `Route` and `Urgent` for the `[jev]` switches that are on.
//! ai.toml is reread when it changes.

mod act;
mod check;
mod config;
mod events;
mod jev;
mod log;
mod ollama;
mod route;
mod senses;
mod server;
mod suggest;

fn main() {
    let config: config::Ai = autark_common::config::load("ai");
    if !config.enabled {
        eprintln!("ai: AI is off; only Check's built-in rules answer (set `enabled = true` in ai.toml)");
    }
    let result = async_io::block_on(async {
        let connection = zbus::connection::Builder::session().name("org.autark.AI1").serve_at(server::PATH, server::Ai::new(config)).build().await?;
        let suggestions = suggest::watch(connection.clone());
        // Settings and hand edits apply at once.
        let ai = connection.object_server().interface::<_, server::Ai>(server::PATH).await?;
        let reloads = async {
            match autark_common::changes::Changes::config("ai") {
                Ok(mut changes) => {
                    while changes.next().await.is_ok() {
                        ai.get().await.reload(autark_common::config::load("ai"));
                    }
                }
                Err(err) => eprintln!("ai: watching ai.toml: {err}"),
            }
            std::future::pending::<()>().await
        };
        futures_util::future::join(reloads, suggestions).await;
        Ok::<(), zbus::Error>(())
    });
    if let Err(err) = result {
        eprintln!("ai: {err}");
        std::process::exit(1);
    }
}
