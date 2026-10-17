//! Kith's local daemon (org.autark.AI1): routing, Jev and Ollama, laptop MCP tools, tasks, memory.
//!
//! So far: `Route`, `Rank`, `Check`, `Ask`, `AskFiles` and `Models`, answered by Ollama; `Act`, which runs
//! routed actions; and the conversation (`Chat`, `ChatAnswer`, `ChatAct`, `Remember`, `History`), which proposes
//! facts for memory.toml. ai.toml is reread when it changes. Jev, tasks and the MCP server come next.

mod act;
mod chat;
mod check;
mod config;
mod files;
mod log;
mod memory;
mod ollama;
mod route;
mod runner;
mod server;
mod tasks;
mod web;

fn main() {
    // `autark-ai task <id>` is one run of a task, as its timer starts it.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [verb, id] = args.as_slice()
        && verb == "task"
    {
        if let Err(err) = runner::run(id) {
            eprintln!("ai: task {id}: {err}");
            std::process::exit(1);
        }
        return;
    }
    let config: config::Ai = autark_common::config::load("ai");
    if !config.enabled {
        eprintln!("ai: AI is off; only Check's built-in rules answer (set `enabled = true` in ai.toml)");
    }
    let result = async_io::block_on(async {
        let connection = zbus::connection::Builder::session().name("org.autark.AI1").serve_at(server::PATH, server::Ai::new(config)).build().await?;
        // Settings and hand edits apply at once.
        let mut changes = match autark_common::changes::Changes::config("ai") {
            Ok(changes) => changes,
            Err(err) => {
                eprintln!("ai: watching ai.toml: {err}");
                return std::future::pending::<zbus::Result<()>>().await;
            }
        };
        let ai = connection.object_server().interface::<_, server::Ai>(server::PATH).await?;
        while changes.next().await.is_ok() {
            ai.get().await.reload(autark_common::config::load("ai"));
        }
        std::future::pending::<zbus::Result<()>>().await
    });
    if let Err(err) = result {
        eprintln!("ai: {err}");
        std::process::exit(1);
    }
}
