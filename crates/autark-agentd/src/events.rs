//! The event log: one JSON line for each thing that happened (an agent made or deleted, a message said, an answer
//! given or failed), kept 14 days in `events.jsonl`, readable only by you. It holds who, what kind, when and the first
//! words of a message, never a whole answer.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde_json::{Value, json};

/// How long events are kept.
const KEPT: u64 = 14 * 24 * 60 * 60;
/// How much of a message the log keeps.
const WORDS: usize = 80;

pub struct Events {
    path: PathBuf,
}

impl Events {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Appends an event.
    pub fn record(&self, time: u64, kind: &str, agent: &str, text: &str) {
        let first: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(WORDS).collect();
        let line = json!({ "time": time, "kind": kind, "agent": agent, "text": first });
        let result = (|| {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&self.path)?;
            writeln!(file, "{line}")
        })();
        if let Err(err) = result {
            eprintln!("agentd: writing the event log: {err}");
        }
    }

    /// The events at or after `since`, as a JSON array.
    pub fn since(&self, since: u64) -> String {
        let events: Vec<Value> = self.read().into_iter().filter(|event| event["time"].as_u64().unwrap_or(0) >= since).collect();
        Value::Array(events).to_string()
    }

    fn read(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.path).unwrap_or_default().lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    /// Drops what is older than 14 days.
    pub fn prune(&self, now: u64) {
        let oldest = now.saturating_sub(KEPT);
        let all = self.read();
        let kept: Vec<&Value> = all.iter().filter(|event| event["time"].as_u64().unwrap_or(0) >= oldest).collect();
        if kept.len() == all.len() {
            return;
        }
        let text: String = kept.iter().map(|event| format!("{event}\n")).collect();
        if let Err(err) = std::fs::write(&self.path, text) {
            eprintln!("agentd: pruning the event log: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(test: &str) -> Events {
        let dir = std::env::temp_dir().join(format!("agentd-events-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Events::new(dir.join("events.jsonl"))
    }

    #[test]
    fn events_are_read_from_a_time() {
        let events = log("since");
        events.record(100, "said", "piper", "hello");
        events.record(200, "answered", "piper", "hi");
        let later: Vec<Value> = serde_json::from_str(&events.since(150)).unwrap();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0]["kind"], "answered");
        let _ = std::fs::remove_dir_all(events.path.parent().unwrap());
    }

    #[test]
    fn only_the_first_words_of_a_message_are_kept() {
        let events = log("words");
        events.record(1, "said", "a", &"word ".repeat(100));
        let kept: Vec<Value> = serde_json::from_str(&events.since(0)).unwrap();
        assert!(kept[0]["text"].as_str().unwrap().chars().count() <= WORDS);
        let _ = std::fs::remove_dir_all(events.path.parent().unwrap());
    }

    #[test]
    fn two_weeks_are_kept_and_no_more() {
        let events = log("prune");
        events.record(10, "old", "a", "");
        events.record(KEPT + 100, "new", "a", "");
        events.prune(KEPT + 200);
        let kept: Vec<Value> = serde_json::from_str(&events.since(0)).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["kind"], "new");
        let _ = std::fs::remove_dir_all(events.path.parent().unwrap());
    }
}
