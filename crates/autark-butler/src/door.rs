//! The door to Alfred: `alfred window` on the owner's server, reached over SSH with a key locked to that one
//! command. Each refresh opens one connection, sends its requests one JSON line each and reads one answer a line.
//! Nothing is kept here: the window shows what comes back and forgets it.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::Deserialize;
use serde_json::Value;

/// `~/.config/autark/butler.toml`: where Alfred lives and the key locked to `alfred window`.
#[derive(Deserialize, Default)]
pub struct Config {
    pub host: Option<String>,
    pub key: Option<String>,
}

pub const NOT_SET: &str = "Not set up: put host and key (locked to `alfred window`) in ~/.config/autark/butler.toml.";

pub struct Door {
    program: String,
    args: Vec<String>,
}

impl Door {
    pub fn new(config: &Config) -> Result<Door, String> {
        let (Some(host), Some(key)) = (&config.host, &config.key) else {
            return Err(NOT_SET.into());
        };
        let options = ["IdentitiesOnly=yes", "BatchMode=yes", "ConnectTimeout=10", "ServerAliveInterval=10"];
        let mut args = vec!["-T".to_owned(), "-i".to_owned(), key.clone()];
        args.extend(options.iter().flat_map(|o| ["-o".to_owned(), (*o).to_owned()]));
        args.extend(["--".to_owned(), host.clone(), "window".to_owned()]);
        Ok(Door { program: "ssh".into(), args })
    }

    /// Each request's answer, in order: its `ok` or its `error`. Err when Alfred couldn't be reached at all.
    pub fn ask(&self, requests: &[Value]) -> Result<Vec<Result<Value, String>>, String> {
        let unreachable = |e: &dyn std::fmt::Display| format!("Couldn't reach Alfred: {e}");
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| unreachable(&e))?;
        let lines: String = requests.iter().map(|r| format!("{r}\n")).collect();
        let sent = child.stdin.take().map(|mut stdin| stdin.write_all(lines.as_bytes()));
        let out = child.wait_with_output().map_err(|e| unreachable(&e))?;
        let answers: Vec<Value> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if !matches!(sent, Some(Ok(()))) || answers.len() != requests.len() {
            return Err(unreachable(&"no answer (is the server up, and the key set up?)"));
        }
        Ok(answers.into_iter().map(|a| match a.get("ok") {
            Some(ok) => Ok(ok.clone()),
            None => Err(a["error"].as_str().unwrap_or("no answer").to_owned()),
        }).collect())
    }
}

/// One turn from the log, for a row: where, which model, tokens, time and how it ended. Never the message.
pub fn turn(t: &Value) -> String {
    let n = |k: &str| t[k].as_i64().unwrap_or(0);
    let s = |k: &str| t[k].as_str().unwrap_or("?").to_owned();
    let tools = Some(s("tools")).filter(|x| !x.is_empty() && x != "?").map(|x| format!(" · {x}")).unwrap_or_default();
    let secs = n("ms") as f64 / 1000.0;
    format!("{} · {} · {} in / {} out · {secs:.1} s · {}{tools}", s("channel"), s("model"), n("tokens_in"), n("tokens_out"), s("outcome"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fake(script: &str) -> Door {
        Door { program: "sh".into(), args: vec!["-c".into(), script.into()] }
    }

    #[test]
    fn requests_go_one_a_line_and_each_answer_comes_back_in_order() {
        let door = fake(r#"while read -r l; do printf '{"ok":%s}\n' "$l"; done"#);
        let got = door.ask(&[json!({"op": "memory"}), json!({"op": "turns", "n": 5})]).unwrap();
        assert_eq!(got, [Ok(json!({"op": "memory"})), Ok(json!({"op": "turns", "n": 5}))]);
        let door = fake(r#"read -r l; echo '{"error":"decide needs id and yes"}'"#);
        assert_eq!(door.ask(&[json!({"op": "decide"})]).unwrap(), [Err("decide needs id and yes".to_owned())]);
    }

    #[test]
    fn a_missing_answer_or_door_is_unreachable_not_an_empty_list() {
        let short = fake(r#"read -r l; echo '{"ok":[]}'"#);
        assert!(short.ask(&[json!({"op": "approvals"}), json!({"op": "memory"})]).is_err());
        let gone = Door { program: "/nonexistent/ssh".into(), args: vec![] };
        assert!(gone.ask(&[json!({"op": "approvals"})]).unwrap_err().starts_with("Couldn't reach Alfred"));
    }

    #[test]
    fn the_door_is_ssh_with_the_locked_key_and_only_the_window_command() {
        assert_eq!(Door::new(&Config::default()).err().as_deref(), Some(NOT_SET));
        let config = Config { host: Some("me@server".into()), key: Some("/k/alfred_window".into()) };
        let door = Door::new(&config).unwrap();
        assert_eq!(door.program, "ssh");
        assert!(door.args.windows(2).any(|w| w == ["-i", "/k/alfred_window"]));
        assert!(door.args.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert_eq!(door.args[door.args.len() - 3..], ["--", "me@server", "window"]);
    }

    #[test]
    fn a_turn_row_shows_numbers_and_never_more_than_the_log_holds() {
        let t = json!({"at": 1, "channel": "telegram", "model": "gpt-oss:20b", "tools": "reminders",
            "tokens_in": 1200, "tokens_out": 80, "ms": 2300, "outcome": "ok"});
        assert_eq!(turn(&t), "telegram · gpt-oss:20b · 1200 in / 80 out · 2.3 s · ok · reminders");
        assert_eq!(turn(&json!({})), "? · ? · 0 in / 0 out · 0.0 s · ?");
    }
}
