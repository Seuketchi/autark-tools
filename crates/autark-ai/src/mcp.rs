//! `autark-ai mcp`: the laptop's tools for an MCP client (an agent in a terminal, say), spoken as JSON-RPC over
//! stdin and stdout, one message a line. Read-only: it reads the files Kith may read and under the same limits,
//! rates a command by the built-in rules, lists what Kith remembers and gives one reading of the system.
//! Nothing that changes the laptop is here unless `[mcp] act = true` adds `run_actions`, the catalog's actions
//! through `Act`. Off until `enabled = true`, and every call is logged like the daemon's.

use std::io::{BufRead, Write};
use std::time::Instant;

use serde_json::{Value, json};

use crate::route::Action;
use crate::{act, check, config, files, log, memory, web};

/// The tools beyond the read-only four that the config offers.
#[derive(Clone, Copy, Default)]
struct Offered {
    act: bool,
    web: bool,
}

const PROTOCOL: &str = "2025-06-18";

#[zbus::proxy(interface = "org.autark.Stats1", default_service = "org.autark.Stats1", default_path = "/org/autark/Stats1")]
trait Stats {
    fn snapshot(&self) -> zbus::Result<String>;
}

/// Serves the client on stdin until it closes.
pub fn serve(config: &config::Ai) {
    let tools = Offered { act: config.mcp.act, web: config.web.search && !config.local_only };
    let enabled = config.enabled;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&message, enabled, tools),
            Err(err) => Some(error(Value::Null, -32700, &format!("not JSON: {err}"))),
        };
        if let Some(reply) = reply
            && (writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err())
        {
            break;
        }
    }
}

/// The reply to one message, or `None` for a notification, which gets none.
fn handle(message: &Value, enabled: bool, offered: Offered) -> Option<Value> {
    let id = message.get("id")?.clone();
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": PROTOCOL,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "autark-ai", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools(offered) }),
        "tools/call" => call(&message["params"], enabled, offered),
        other => return Some(error(id, -32601, &format!("no method “{other}”"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools(offered: Offered) -> Value {
    let mut tools = json!([
        {
            "name": "read_file",
            "description": "Reads a file in the user's home folder (never a dotfile or anything in a dot-folder): at most 8 KB, the end of a long one.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "A path such as ~/notes/todo.md" } }, "required": ["path"] },
        },
        {
            "name": "check_command",
            "description": "Says whether a shell command is destructive by Kith's built-in rules (rm -rf, dd, mkfs, git push --force and the like), without running it.",
            "inputSchema": { "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] },
        },
        {
            "name": "memory",
            "description": "What Kith remembers about the user: the facts they saved.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "system_stats",
            "description": "One reading of the laptop: CPU, memory, disk and network rates, the battery, and each app's CPU, memory and power.",
            "inputSchema": { "type": "object", "properties": {} },
        },
    ]);
    if offered.web {
        tools.as_array_mut().into_iter().for_each(|tools| tools.push(json!({
            "name": "web_search",
            "description": "Searches the web through Firecrawl and returns the best results' titles, addresses and snippets. The query leaves the laptop.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] },
        })));
    }
    if offered.act {
        tools.as_array_mut().into_iter().for_each(|tools| tools.push(json!({
            "name": "run_actions",
            "description": "Changes the laptop through Kith's catalog, without asking the user: set_volume and set_brightness (percent), mute, wifi, focus and night_light (on), lock, sleep, open_app (name), open_url (url). Lock and sleep run last.",
            "inputSchema": { "type": "object", "properties": { "actions": { "type": "array", "items": { "type": "object", "properties": { "action": { "type": "string" } }, "required": ["action"] }, "description": "e.g. [{\"action\":\"mute\",\"on\":true},{\"action\":\"set_volume\",\"percent\":30}]" } }, "required": ["actions"] },
        })));
    }
    tools
}

/// A `tools/call`: the tool's text, or why it failed, as the protocol's `isError` result.
fn call(params: &Value, enabled: bool, offered: Offered) -> Value {
    let name = params["name"].as_str().unwrap_or_default();
    let arguments = &params["arguments"];
    let start = Instant::now();
    let result = if enabled { run(name, arguments, offered) } else { Err("AI is off: set `enabled = true` in ai.toml".to_owned()) };
    log::record(&format!("mcp:{name}"), "mcp", arguments.clone(), &result.clone().map(|text| json!({ "bytes": text.len() })), start.elapsed());
    match result {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
    }
}

fn run(name: &str, arguments: &Value, offered: Offered) -> Result<String, String> {
    let argument = |key: &str| arguments[key].as_str().filter(|text| !text.is_empty()).ok_or_else(|| format!("{name} needs “{key}”"));
    match name {
        "read_file" => {
            let (text, outcome) = files::read(&[argument("path")?.to_owned()]);
            match outcome.first() {
                Some((_, reason)) if !reason.is_empty() => Err(reason.clone()),
                _ => Ok(text),
            }
        }
        "check_command" => Ok(match check::rules(argument("command")?) {
            Some(reason) => format!("Destructive: {reason}."),
            None => "Nothing in the built-in rules flags it. That is not a promise it is safe.".to_owned(),
        }),
        "memory" => {
            let facts = memory::facts();
            Ok(if facts.is_empty() { "Nothing is remembered yet.".to_owned() } else { facts.iter().map(|fact| format!("- {fact}")).collect::<Vec<_>>().join("\n") })
        }
        "system_stats" => async_io::block_on(async {
            let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
            let stats = StatsProxy::new(&connection).await.map_err(|err| format!("autark-stats: {err}"))?;
            stats.snapshot().await.map_err(|err| format!("autark-stats: {err}"))
        }),
        "web_search" if offered.web => async_io::block_on(async {
            let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
            let key = web::key(&connection).await?;
            let query = argument("query")?.to_owned();
            blocking::unblock(move || web::search(&key, &query)).await
        }),
        "run_actions" if offered.act => {
            let actions: Vec<Action> = serde_json::from_value(arguments["actions"].clone()).map_err(|err| format!("not actions from the catalog: {err}"))?;
            // The three a suggestion offers (quit an app, open a file, open a pane) aren't in the catalog.
            if actions.iter().any(|action| matches!(action, Action::QuitApp { .. } | Action::OpenFile { .. } | Action::OpenSettings { .. })) {
                return Err("only the catalog's actions run here".to_owned());
            }
            async_io::block_on(async {
                let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
                let results = act::run(&connection, &actions).await;
                Ok(results.iter().map(|why| if why.is_empty() { "done".to_owned() } else { why.clone() }).collect::<Vec<_>>().join("\n"))
            })
        }
        other => Err(format!("no tool “{other}”")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(method: &str, params: Value) -> Value {
        handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }), true, Offered::default()).unwrap()
    }

    #[test]
    fn it_introduces_itself_and_lists_its_tools() {
        let hello = ask("initialize", json!({}));
        assert_eq!(hello["result"]["serverInfo"]["name"], "autark-ai");
        let tools = ask("tools/list", json!({}));
        let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
        assert_eq!(names, ["read_file", "check_command", "memory", "system_stats"]);
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_an_error() {
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), true, Offered::default()).is_none());
        assert_eq!(ask("resources/list", json!({}))["error"]["code"], -32601);
    }

    #[test]
    fn acting_is_offered_only_when_allowed() {
        assert_eq!(tools(Offered::default()).as_array().unwrap().len(), 4);
        assert_eq!(tools(Offered { act: true, web: true })[4]["name"], "web_search");
        assert_eq!(tools(Offered { act: true, web: false })[4]["name"], "run_actions");
        let call_run = |act| call(&json!({ "name": "run_actions", "arguments": { "actions": [{ "action": "quit_app", "app": "x" }] } }), true, Offered { act, web: false });
        assert_eq!(call_run(false)["content"][0]["text"], "no tool “run_actions”");
        assert_eq!(call_run(true)["content"][0]["text"], "only the catalog's actions run here");
    }

    #[test]
    fn tools_answer_or_say_why_not() {
        let rm = ask("tools/call", json!({ "name": "check_command", "arguments": { "command": "rm -rf build" } }));
        assert_eq!(rm["result"]["isError"], false);
        assert!(rm["result"]["content"][0]["text"].as_str().unwrap().starts_with("Destructive"));
        let secret = ask("tools/call", json!({ "name": "read_file", "arguments": { "path": "~/.ssh/id_ed25519" } }));
        assert_eq!(secret["result"]["isError"], true);
        let missing = ask("tools/call", json!({ "name": "check_command", "arguments": {} }));
        assert_eq!(missing["result"]["isError"], true);
        let off = handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "memory" } }), false, Offered::default()).unwrap();
        assert_eq!(off["result"]["isError"], true);
    }
}
