//! An agent's conversation: what you said and what it answered, kept a week in its own `chat.json`, readable only by
//! you. The entries are shaped as Kith's own are (`kind` is `you` or `answer`), so a client reads both the same way.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How long the conversation is kept.
const KEPT: u64 = 7 * 24 * 60 * 60;
/// How many entries go back to the model as the conversation so far.
const RECENT: usize = 12;
/// The version of `chat.json` this writes.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub id: u64,
    /// Seconds since the Unix epoch.
    pub time: u64,
    #[serde(flatten)]
    pub what: What,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
    /// A job the agent proposes to run on a schedule because you asked for something every day or week, for you to
    /// schedule or not.
    Job { when: String, prompt: String, state: FactState },
    /// What a scheduled job asked, as if you had said it: `when` is the job's words.
    Scheduled { when: String, prompt: String },
    /// A handoff this agent made: work it asked another agent to do, or why it couldn't (`refused`, empty when it went).
    Handoff { to: String, name: String, brief: String, refused: String },
    /// A message from another agent, written here by the daemon: what it asked this agent to do, or (`reply`) its answer
    /// to what this agent asked it.
    From { from: String, name: String, text: String, reply: bool },
    /// Lines the agent proposes to add to the end of one of your notes because you asked it to, for you to add or not.
    Add { note: String, items: Vec<String>, state: FactState },
    /// A fact the agent proposes to keep in its memory because you asked it to remember something, for you to save or not.
    Fact { text: String, state: FactState },
    /// What you said.
    You { text: String },
    /// The agent's answer, its text growing while `writing`. `answer` is the model's answer id, 0 until it starts.
    Answer {
        answer: u32,
        text: String,
        #[serde(default)]
        writing: bool,
        #[serde(default)]
        error: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FactState {
    Proposed,
    Saved,
    Dismissed,
}

pub struct Chat {
    entries: Vec<Entry>,
    /// Entries of a kind this version doesn't know, written by a newer one: kept as they are and saved again.
    foreign: Vec<Value>,
    next: u64,
    /// Where it is saved; `None` when it mustn't be, as for a file a newer version wrote.
    path: Option<PathBuf>,
    /// Who answers in it, for the transcript.
    speaker: String,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

impl Chat {
    /// A conversation kept at `path`, with `speaker` answering.
    /// A file that can't be understood is set aside and a new conversation begun; one that can't be read, or that a
    /// newer version wrote, is shown but never written over.
    pub fn load(path: PathBuf, speaker: &str) -> Self {
        let ((values, saved_next), path) = match read(&path) {
            Ok(Some(stored)) if stored.version > VERSION => {
                eprintln!("alfred: {} is from a newer version (format {}), so it is shown but not changed", path.display(), stored.version);
                ((stored.entries, stored.next), None)
            }
            Ok(Some(stored)) => ((stored.entries, stored.next), Some(path)),
            Ok(None) => ((Vec::new(), 0), Some(path)),
            Err(Unread::Unreadable(err)) => {
                eprintln!("alfred: reading {}: {err}; it is left as it is", path.display());
                ((Vec::new(), 0), None)
            }
            Err(Unread::NotUnderstood(err)) => {
                match crate::files::set_aside(&path, now()) {
                    Ok(aside) => eprintln!("alfred: {} can't be understood ({err}); it was kept as {}", path.display(), aside.display()),
                    Err(why) => eprintln!("alfred: {} can't be understood ({err}) or set aside ({why})", path.display()),
                }
                let saved = !path.exists();
                ((Vec::new(), 0), saved.then_some(path))
            }
        };
        let mut entries = Vec::new();
        let mut foreign = Vec::new();
        for value in values {
            match serde_json::from_value::<Entry>(value.clone()) {
                Ok(entry) => entries.push(entry),
                Err(_) => foreign.push(value),
            }
        }
        // An id is never given twice, even after a clear or a restart, so a client holding one never reaches another entry.
        let next = (entries.iter().map(|entry| entry.id).chain(foreign.iter().filter_map(|value| value["id"].as_u64())).max().unwrap_or(0) + 1).max(saved_next);
        let mut chat = Self { entries, foreign, next, path, speaker: speaker.to_owned() };
        // An answer cut off by a restart won't grow any more.
        for entry in &mut chat.entries {
            if let What::Answer { writing, error, .. } = &mut entry.what
                && *writing
            {
                *writing = false;
                if error.is_empty() {
                    *error = "This answer was cut off when the team's backend stopped.".to_owned();
                }
            }
        }
        chat.prune();
        chat
    }

    #[cfg(test)]
    pub fn in_memory(speaker: &str) -> Self {
        Self { entries: Vec::new(), foreign: Vec::new(), next: 1, path: None, speaker: speaker.to_owned() }
    }

    fn prune(&mut self) {
        let oldest = now().saturating_sub(KEPT);
        self.entries.retain(|entry| entry.time >= oldest);
        self.foreign.retain(|value| value["time"].as_u64().unwrap_or(0) >= oldest);
    }

    pub fn entries(&mut self) -> &[Entry] {
        self.prune();
        &self.entries
    }

    pub fn push(&mut self, what: What) -> u64 {
        let id = self.next;
        self.next += 1;
        self.entries.push(Entry { id, time: now(), what });
        id
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut What> {
        self.entries.iter_mut().find(|entry| entry.id == id).map(|entry| &mut entry.what)
    }

    /// The proposed job at `entry`, if it is one.
    pub fn job_mut(&mut self, entry: u64) -> Option<(&str, &str, &mut FactState)> {
        match self.get_mut(entry)? {
            What::Job { when, prompt, state } => Some((when.as_str(), prompt.as_str(), state)),
            _ => None,
        }
    }

    /// The proposed addition to a note at `entry`, if it is one.
    pub fn add_mut(&mut self, entry: u64) -> Option<(&str, &[String], &mut FactState)> {
        match self.get_mut(entry)? {
            What::Add { note, items, state } => Some((note.as_str(), items.as_slice(), state)),
            _ => None,
        }
    }

    /// The proposed fact at `entry`, if it is one.
    pub fn fact_mut(&mut self, entry: u64) -> Option<(&str, &mut FactState)> {
        match self.get_mut(entry)? {
            What::Fact { text, state } => Some((text.as_str(), state)),
            _ => None,
        }
    }

    /// The answer being written under model answer `answer`.
    pub fn answer_mut(&mut self, answer: u32) -> Option<&mut What> {
        self.entries.iter_mut().rev().find(|entry| matches!(entry.what, What::Answer { answer: id, writing: true, .. } if id == answer)).map(|entry| &mut entry.what)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.foreign.clear();
    }

    /// What was said last, by you or the agent, and when: for the roster's preview.
    pub fn last_said(&mut self) -> (String, u64) {
        self.prune();
        self.entries
            .iter()
            .rev()
            .find_map(|entry| match &entry.what {
                What::You { text } | What::Scheduled { prompt: text, .. } => Some((text.clone(), entry.time)),
                What::Answer { text, .. } | What::From { text, .. } if !text.is_empty() => Some((text.clone(), entry.time)),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Whether an answer is being written.
    pub fn writing(&self) -> bool {
        self.entries.iter().any(|entry| matches!(entry.what, What::Answer { writing: true, .. }))
    }

    /// The recent conversation, for the model.
    pub fn transcript(&self) -> String {
        let start = self.entries.len().saturating_sub(RECENT);
        let mut lines = Vec::new();
        for entry in &self.entries[start..] {
            match &entry.what {
                What::You { text } => lines.push(format!("User: {text}")),
                What::Scheduled { prompt, .. } => lines.push(format!("User (a standing request, run on a schedule): {prompt}")),
                What::Answer { text, .. } if !text.is_empty() => lines.push(format!("{}: {text}", self.speaker)),
                What::From { name, text, reply, .. } => lines.push(if *reply { format!("{name} replied: {text}") } else { format!("{name} (an agent, not the user): {text}") }),
                _ => {}
            }
        }
        lines.join("\n")
    }

    /// Writes the conversation whole, only you able to read it.
    pub fn save(&mut self) {
        self.prune();
        let Some(path) = self.path.clone() else { return };
        let result = (|| {
            let mut entries = self.entries.iter().map(serde_json::to_value).collect::<Result<Vec<Value>, _>>()?;
            entries.extend(self.foreign.iter().cloned());
            entries.sort_by_key(|entry| entry["id"].as_u64().unwrap_or(0));
            let bytes = serde_json::to_vec(&json!({ "version": VERSION, "next": self.next, "entries": entries }))?;
            crate::files::write(&path, &bytes)
        })();
        if let Err(err) = result {
            eprintln!("alfred: saving the conversation to {}: {err}", path.display());
        }
    }
}

/// Why a conversation's file wasn't read.
enum Unread {
    Unreadable(std::io::Error),
    NotUnderstood(String),
}

/// A conversation's file as it is kept: its entries unparsed.
struct Stored {
    version: u32,
    /// The next entry's id, 0 when the file doesn't say.
    next: u64,
    entries: Vec<Value>,
}

/// The file, `None` when there is none. The first files were a bare array.
fn read(path: &std::path::Path) -> Result<Option<Stored>, Unread> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Unread::Unreadable(err)),
    };
    match serde_json::from_slice(&bytes).map_err(|err| Unread::NotUnderstood(err.to_string()))? {
        Value::Array(entries) => Ok(Some(Stored { version: 1, next: 0, entries })),
        Value::Object(mut file) => {
            let version = file.get("version").and_then(Value::as_u64).unwrap_or(1).try_into().unwrap_or(u32::MAX);
            let next = file.get("next").and_then(Value::as_u64).unwrap_or(0);
            match file.remove("entries") {
                Some(Value::Array(entries)) => Ok(Some(Stored { version, next, entries })),
                _ => Err(Unread::NotUnderstood("it has no entries".to_owned())),
            }
        }
        _ => Err(Unread::NotUnderstood("it is not a conversation".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transcript_names_who_spoke() {
        let mut chat = Chat::in_memory("PIPER");
        chat.push(What::You { text: "any news?".into() });
        chat.push(What::Answer { answer: 1, text: "None.".into(), writing: false, error: String::new() });
        chat.push(What::Answer { answer: 2, text: String::new(), writing: true, error: String::new() });
        assert_eq!(chat.transcript(), "User: any news?\nPIPER: None.");
        assert!(chat.writing());
    }

    #[test]
    fn the_last_words_are_the_last_that_say_anything() {
        let mut chat = Chat::in_memory("A");
        assert_eq!(chat.last_said().0, "");
        chat.push(What::You { text: "hi".into() });
        chat.push(What::Answer { answer: 1, text: String::new(), writing: true, error: String::new() });
        assert_eq!(chat.last_said().0, "hi");
    }

    #[test]
    fn entries_are_shaped_as_kiths_are() {
        let mut chat = Chat::in_memory("A");
        chat.push(What::You { text: "hi".into() });
        chat.push(What::Answer { answer: 7, text: "Hello.".into(), writing: false, error: String::new() });
        let json = serde_json::to_value(chat.entries()).unwrap();
        assert_eq!(json[0]["kind"], "you");
        assert_eq!(json[1]["kind"], "answer");
        assert_eq!(json[1]["answer"], 7);
        assert_eq!(json[1]["writing"], false);
    }

    #[test]
    fn an_answer_cut_off_by_a_restart_says_so() {
        let dir = std::env::temp_dir().join(format!("alfred-chat-{}", std::process::id()));
        let path = dir.join("chat.json");
        let mut chat = Chat::load(path.clone(), "A");
        chat.push(What::Answer { answer: 3, text: "Par".into(), writing: true, error: String::new() });
        chat.save();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let mut again = Chat::load(path, "A");
        assert!(!again.writing());
        assert!(matches!(&again.entries()[0].what, What::Answer { error, text, .. } if error.contains("cut off") && text == "Par"));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn place(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alfred-chat-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("chat.json")
    }

    #[test]
    fn an_entry_of_a_kind_not_known_here_is_kept() {
        let path = place("unknown");
        let now = now();
        std::fs::write(&path, format!(r#"[{{"id":1,"time":{now},"kind":"you","text":"hi"}},{{"id":2,"time":{now},"kind":"handoff","to":"piper"}}]"#)).unwrap();
        let mut chat = Chat::load(path.clone(), "A");
        assert_eq!(chat.entries().len(), 1);
        assert_eq!(chat.push(What::You { text: "again".into() }), 3);
        chat.save();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], VERSION);
        let kinds: Vec<&str> = saved["entries"].as_array().unwrap().iter().map(|entry| entry["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["you", "handoff", "you"]);
        assert_eq!(saved["entries"][1]["to"], "piper");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_entry_id_is_not_given_again_after_a_clear_and_a_restart() {
        let path = place("ids");
        let mut chat = Chat::load(path.clone(), "A");
        for _ in 0..3 {
            chat.push(What::You { text: "hi".into() });
        }
        chat.clear();
        chat.save();
        assert_eq!(Chat::load(path.clone(), "A").push(What::You { text: "again".into() }), 4);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_conversation_that_cant_be_understood_is_set_aside_not_written_over() {
        let path = place("bad");
        std::fs::write(&path, "{ not json").unwrap();
        let mut chat = Chat::load(path.clone(), "A");
        assert!(chat.entries().is_empty());
        chat.push(What::You { text: "hi".into() });
        chat.save();
        let aside: Vec<PathBuf> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().map(|entry| entry.path()).filter(|p| p.to_string_lossy().contains("chat.json.bad-")).collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "{ not json");
        assert_eq!(Chat::load(path.clone(), "A").entries().len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_conversation_from_a_newer_version_is_read_but_not_written_over() {
        let path = place("newer");
        let text = format!(r#"{{"version":{},"entries":[{{"id":1,"time":{},"kind":"you","text":"hi"}}]}}"#, VERSION + 1, now());
        std::fs::write(&path, &text).unwrap();
        let mut chat = Chat::load(path.clone(), "A");
        assert_eq!(chat.entries().len(), 1);
        chat.push(What::You { text: "more".into() });
        chat.save();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_proposed_fact_is_an_entry_but_not_part_of_what_was_said() {
        let mut chat = Chat::in_memory("A");
        chat.push(What::You { text: "remember that I like oat milk".into() });
        let entry = chat.push(What::Fact { text: "I like oat milk".into(), state: FactState::Proposed });
        assert_eq!(chat.transcript(), "User: remember that I like oat milk");
        assert_eq!(chat.last_said().0, "remember that I like oat milk");
        let json = serde_json::to_value(chat.entries()).unwrap();
        assert_eq!((json[1]["kind"].as_str(), json[1]["state"].as_str()), (Some("fact"), Some("proposed")));
        assert!(chat.fact_mut(entry).is_some() && chat.fact_mut(1).is_none());
    }

    #[test]
    fn what_agents_say_to_each_other_is_in_the_conversation_and_the_transcript() {
        let mut chat = Chat::in_memory("RAVEN");
        chat.push(What::From { from: "atlas".into(), name: "ATLAS".into(), text: "Forecast for Cebu?".into(), reply: false });
        chat.push(What::Answer { answer: 1, text: "Rain.".into(), writing: false, error: String::new() });
        chat.push(What::Handoff { to: "quill".into(), name: "QUILL".into(), brief: "x".into(), refused: String::new() });
        assert_eq!(chat.transcript(), "ATLAS (an agent, not the user): Forecast for Cebu?\nRAVEN: Rain.");
        assert_eq!(chat.last_said().0, "Rain.");
        let json = serde_json::to_value(chat.entries()).unwrap();
        assert_eq!((json[0]["kind"].as_str(), json[0]["reply"].as_bool(), json[2]["kind"].as_str()), (Some("from"), Some(false), Some("handoff")));
    }

    #[test]
    fn a_scheduled_request_reads_as_the_user_asking() {
        let mut chat = Chat::in_memory("ATLAS");
        chat.push(What::Scheduled { when: "weekdays at 8:00".into(), prompt: "Summarise my calendar".into() });
        chat.push(What::Answer { answer: 1, text: "Nothing today.".into(), writing: false, error: String::new() });
        chat.push(What::Job { when: "every day at 9:00".into(), prompt: "x".into(), state: FactState::Proposed });
        assert_eq!(chat.transcript(), "User (a standing request, run on a schedule): Summarise my calendar\nATLAS: Nothing today.");
        assert_eq!(chat.last_said().0, "Nothing today.");
        let json = serde_json::to_value(chat.entries()).unwrap();
        assert_eq!((json[0]["kind"].as_str(), json[2]["kind"].as_str(), json[2]["state"].as_str()), (Some("scheduled"), Some("job"), Some("proposed")));
        assert!(chat.job_mut(3).is_some() && chat.job_mut(1).is_none());
    }

    #[test]
    fn a_week_is_kept() {
        let mut chat = Chat::in_memory("A");
        chat.push(What::You { text: "old".into() });
        chat.push(What::You { text: "new".into() });
        chat.entries[0].time = now() - KEPT - 60;
        assert_eq!(chat.entries().len(), 1);
    }
}
