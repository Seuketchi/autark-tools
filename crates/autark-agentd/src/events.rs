//! The event log: one JSON line for each thing that happened (an agent made or deleted, a message said, an answer
//! given or failed), kept 14 days in `events.jsonl`, readable only by you. It holds who, what kind, when and the first
//! words of what you said; of an answer only its length, never its words. Each event has a number, `seq`, larger than
//! any before it, so a client can ask for what came after the last one it saw.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde_json::{Value, json};

/// How long events are kept.
const KEPT: u64 = 14 * 24 * 60 * 60;
/// How much of a message the log keeps.
const WORDS: usize = 80;

pub struct Events {
    path: PathBuf,
    /// The last event's number. Held while the file is written, so numbers go in order and a prune loses none.
    last: Mutex<u64>,
}

impl Events {
    pub fn new(path: PathBuf) -> Self {
        let events = Self { path, last: Mutex::new(0) };
        let last = events.read().iter().filter_map(|event| event["seq"].as_u64()).max().unwrap_or(0);
        *events.lock() = last;
        events
    }

    fn lock(&self) -> MutexGuard<'_, u64> {
        self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Appends an event.
    pub fn record(&self, time: u64, kind: &str, agent: &str, text: &str) {
        let first: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(WORDS).collect();
        let mut last = self.lock();
        // A thousand a second from the time, so numbers keep growing when the log has been pruned empty and the daemon
        // started again; one more than the last when they come faster.
        let seq = (*last + 1).max(time.saturating_mul(1000));
        let line = json!({ "seq": seq, "time": time, "kind": kind, "agent": agent, "text": first });
        let result = (|| {
            if let Some(dir) = self.path.parent() {
                crate::files::private_dir(dir)?;
            }
            let mut file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&self.path)?;
            writeln!(file, "{line}")
        })();
        match result {
            Ok(()) => *last = seq,
            Err(err) => eprintln!("agentd: writing the event log: {err}"),
        }
    }

    /// The events at or after `since`, as a JSON array.
    pub fn since(&self, since: u64) -> String {
        let events: Vec<Value> = self.read().into_iter().filter(|event| event["time"].as_u64().unwrap_or(0) >= since).collect();
        Value::Array(events).to_string()
    }

    /// The events numbered after `seq`, as a JSON array.
    pub fn after(&self, seq: u64) -> String {
        let events: Vec<Value> = self.read().into_iter().filter(|event| event["seq"].as_u64().unwrap_or(0) > seq).collect();
        Value::Array(events).to_string()
    }

    fn read(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.path).unwrap_or_default().lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    /// Drops what is older than 14 days, writing the log whole, with no event recorded meanwhile.
    pub fn prune(&self, now: u64) {
        let _last = self.lock();
        let oldest = now.saturating_sub(KEPT);
        let all = self.read();
        let kept: Vec<&Value> = all.iter().filter(|event| event["time"].as_u64().unwrap_or(0) >= oldest).collect();
        if kept.len() == all.len() {
            return;
        }
        let text: String = kept.iter().map(|event| format!("{event}\n")).collect();
        if let Err(err) = crate::files::write(&self.path, text.as_bytes()) {
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
    fn each_event_has_a_number_that_only_grows_even_across_a_restart() {
        let events = log("seq");
        events.record(100, "said", "piper", "a");
        events.record(100, "answered", "piper", "b");
        let all: Vec<Value> = serde_json::from_str(&events.since(0)).unwrap();
        let first = all[0]["seq"].as_u64().unwrap();
        let second = all[1]["seq"].as_u64().unwrap();
        assert!(second > first);
        // Two events in the same second are told apart by their numbers.
        let after: Vec<Value> = serde_json::from_str(&events.after(first)).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["kind"], "answered");
        let again = Events::new(events.path.clone());
        again.record(50, "said", "piper", "c");
        let after: Vec<Value> = serde_json::from_str(&again.after(second)).unwrap();
        assert_eq!(after.len(), 1);
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

    #[test]
    fn a_pruned_log_is_written_whole_and_only_yours() {
        use std::os::unix::fs::PermissionsExt;
        let events = log("prune-mode");
        events.record(10, "old", "a", "");
        events.record(KEPT + 100, "new", "a", "");
        std::fs::set_permissions(&events.path, std::fs::Permissions::from_mode(0o644)).unwrap();
        events.prune(KEPT + 200);
        assert_eq!(std::fs::metadata(&events.path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(events.path.parent().unwrap()).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(events.path.parent().unwrap());
    }
}
