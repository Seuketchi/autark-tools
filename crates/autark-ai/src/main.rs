//! Kith's local daemon (org.autark.AI1): routing, Jev and Ollama, laptop MCP tools, tasks, memory.
//!
//! So far: `Route`, `Rank`, `Check`, `Ask`, `AskFiles` and `Models`, answered by Ollama; `Act`, which runs
//! routed actions; and the conversation (`Chat`, `ChatAnswer`, `ChatAct`, `Remember`, `History`), which proposes
//! facts for memory.toml and tasks; `autark-ai task <id>` runs one. Suggestions come from what code notices
//! (`events`), rated by Ollama (`suggest`). ai.toml is reread when it changes. Jev answers `Check`, `Route`, `Rank` and `Urgent` for the `[jev]` switches that are on; `autark-ai mcp` is the MCP server.

mod act;
mod agent;
mod chat;
mod check;
mod config;
mod events;
mod files;
mod jev;
mod log;
mod mcp;
mod memory;
mod ollama;
mod recall;
mod route;
mod runner;
mod senses;
mod skills;
mod server;
mod soul;
mod suggest;
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
    // `autark-ai mcp` serves an MCP client on stdin and stdout, as the client starts it.
    if args == ["mcp"] {
        mcp::serve(&config);
        return;
    }
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
