//! An agent's conversation: what you said and what it answered, kept a week in its own `chat.json`, readable only by
//! you. The entries are shaped as Kith's own are (`kind` is `you` or `answer`), so a client reads both the same way.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long the conversation is kept.
const KEPT: u64 = 7 * 24 * 60 * 60;
/// How many entries go back to the model as the conversation so far.
const RECENT: usize = 12;

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
    next: u64,
    path: Option<PathBuf>,
    /// Who answers in it, for the transcript.
    speaker: String,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

impl Chat {
    /// A conversation kept at `path`, with `speaker` answering.
    pub fn load(path: PathBuf, speaker: &str) -> Self {
        let entries: Vec<Entry> = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        let next = entries.iter().map(|entry| entry.id + 1).max().unwrap_or(1);
        let mut chat = Self { entries, next, path: Some(path), speaker: speaker.to_owned() };
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
        Self { entries: Vec::new(), next: 1, path: None, speaker: speaker.to_owned() }
    }

    fn prune(&mut self) {
        let oldest = now().saturating_sub(KEPT);
        self.entries.retain(|entry| entry.time >= oldest);
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
    }

    /// What was said last, by you or the agent, and when: for the roster's preview.
    pub fn last_said(&mut self) -> (String, u64) {
        self.prune();
        self.entries
            .iter()
            .rev()
            .find_map(|entry| match &entry.what {
                What::You { text } => Some((text.clone(), entry.time)),
                What::Answer { text, .. } if !text.is_empty() => Some((text.clone(), entry.time)),
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
                What::Answer { text, .. } if !text.is_empty() => lines.push(format!("{}: {text}", self.speaker)),
                _ => {}
            }
        }
        lines.join("\n")
    }

    /// Writes the conversation through a temporary file, only you able to read it.
    pub fn save(&mut self) {
        self.prune();
        let Some(path) = self.path.clone() else { return };
        let result = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let temporary = path.with_extension("json.saving");
            std::fs::write(&temporary, serde_json::to_vec(&self.entries)?)?;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
            std::fs::rename(&temporary, &path)
        })();
        if let Err(err) = result {
            eprintln!("agentd: saving the conversation to {}: {err}", path.display());
        }
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
        let dir = std::env::temp_dir().join(format!("agentd-chat-{}", std::process::id()));
        let path = dir.join("chat.json");
        let mut chat = Chat::load(path.clone(), "A");
        chat.push(What::Answer { answer: 3, text: "Par".into(), writing: true, error: String::new() });
        chat.save();
        let mut again = Chat::load(path, "A");
        assert!(!again.writing());
        assert!(matches!(&again.entries()[0].what, What::Answer { error, text, .. } if error.contains("cut off") && text == "Par"));
        let _ = std::fs::remove_dir_all(dir);
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
    fn a_week_is_kept() {
        let mut chat = Chat::in_memory("A");
        chat.push(What::You { text: "old".into() });
        chat.push(What::You { text: "new".into() });
        chat.entries[0].time = now() - KEPT - 60;
        assert_eq!(chat.entries().len(), 1);
    }
}
