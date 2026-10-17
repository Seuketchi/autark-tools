//! Every AI call, appended as one JSON line to `$XDG_STATE_HOME/autark-ai/log`, so a wrong
//! decision can be traced back to what was asked.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

pub fn record(call: &str, engine: &str, input: Value, answer: &Result<Value, String>, took: Duration) {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (answer, error) = match answer {
        Ok(answer) => (answer.clone(), Value::Null),
        Err(error) => (Value::Null, Value::from(error.as_str())),
    };
    let line = json!({ "time": time, "call": call, "engine": engine, "input": input, "answer": answer, "error": error, "ms": took.as_millis() as u64 });
    if let Err(err) = append(&line.to_string()) {
        eprintln!("ai: writing the call log: {err}");
    }
}

fn append(line: &str) -> std::io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // The log holds what you typed, so only you can read it.
    let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    writeln!(file, "{line}")
}

fn path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("autark-ai/log")
}
