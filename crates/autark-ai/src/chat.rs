//! Kith's conversation: one running thread, kept a week in `$XDG_STATE_HOME/autark-ai/chat.json` and
//! readable only by you. Ask Kith's exchanges go into it as well as the Assistant panel's, so a follow-up
//! carries on from what was asked in the launcher.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
    /// What you said.
    You { text: String },
    /// The actions it asked for, as `Act` takes them. `results` is empty until they run, then holds each
    /// one's outcome: null for not chosen, "" for done, else why not. A newer message skips ones never run.
    Actions {
        actions: Vec<Value>,
        /// How sure Kith was of each, so the panel leaves unsure ones unticked as Ask Kith does.
        #[serde(default)]
        confidence: Vec<f64>,
        #[serde(default)]
        results: Vec<Option<String>>,
        #[serde(default)]
        skipped: bool,
    },
    /// Kith's answer, its text growing while `writing`, with the files it read: path and why not, empty if read.
    Answer {
        answer: u32,
        text: String,
        #[serde(default)]
        files: Vec<(String, String)>,
        #[serde(default)]
        writing: bool,
        #[serde(default)]
        error: String,
    },
    /// A fact Kith proposes to remember.
    Memory { text: String, state: Proposal },
    /// A task Kith proposes; once created, its timer and its first run in microseconds since the epoch.
    Task {
        task: crate::tasks::Task,
        state: TaskState,
        #[serde(default)]
        unit: String,
        #[serde(default)]
        next: u64,
    },
    /// A suggestion Kith made from something it noticed, with its one offer (an action as `Act` takes it,
    /// and its button) and, once answered, how its offer went: empty for done, else why not.
    Suggestion {
        /// What kind of event it came from.
        event: crate::events::Kind,
        /// What "Don't suggest again" covers, and how Settings lists it.
        key: String,
        label: String,
        text: String,
        offer: Option<Value>,
        #[serde(default)]
        offer_label: String,
        /// How the model rated it, 0 to 1.
        worth: f64,
        state: SuggestionState,
        #[serde(default)]
        result: String,
        /// Its quiet notification in the centre, closed once it's answered.
        #[serde(default)]
        notification: u32,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionState {
    Open,
    Accepted,
    NotNow,
    Never,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Proposed,
    Created,
    Cancelled,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Proposal {
    Proposed,
    Saved,
    NotSaved,
}

#[derive(Default)]
pub struct Chat {
    entries: Vec<Entry>,
    next: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("autark-ai/chat.json")
}

impl Chat {
    pub fn load() -> Self {
        let entries: Vec<Entry> = std::fs::read(path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        let next = entries.iter().map(|entry| entry.id + 1).max().unwrap_or(1);
        let mut chat = Self { entries, next };
        // An answer cut off by a restart won't grow any more.
        for entry in &mut chat.entries {
            if let What::Answer { writing, .. } = &mut entry.what {
                *writing = false;
            }
        }
        chat.prune();
        chat
    }

    fn prune(&mut self) {
        let oldest = now().saturating_sub(KEPT);
        self.entries.retain(|entry| entry.time >= oldest);
    }

    pub fn entries(&mut self) -> &[Entry] {
        self.prune();
        &self.entries
    }

    /// A new message: actions from earlier ones that never ran won't run now.
    pub fn said(&mut self, text: &str) -> u64 {
        for entry in &mut self.entries {
            if let What::Actions { results, skipped, .. } = &mut entry.what
                && results.is_empty()
            {
                *skipped = true;
            }
        }
        self.push(What::You { text: text.to_owned() })
    }

    pub fn push(&mut self, what: What) -> u64 {
        let id = self.next;
        self.next += 1;
        self.entries.push(Entry { id, time: now(), what });
        id
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut What> {
        self.entries.iter_mut().find(|entry| entry.id == id).map(|entry| &mut entry.what)
    }

    /// The answer entry streaming answer `answer`.
    pub fn answer_mut(&mut self, answer: u32) -> Option<&mut What> {
        self.entries.iter_mut().rev().find(|entry| matches!(entry.what, What::Answer { answer: id, writing: true, .. } if id == answer)).map(|entry| &mut entry.what)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The recent conversation, for the model: what you said, what ran and what Kith answered.
    pub fn transcript(&self) -> String {
        let start = self.entries.len().saturating_sub(RECENT);
        let mut lines = Vec::new();
        for entry in &self.entries[start..] {
            match &entry.what {
                What::You { text } => lines.push(format!("User: {text}")),
                What::Answer { text, .. } if !text.is_empty() => lines.push(format!("Kith: {text}")),
                What::Actions { actions, results, .. } if !results.is_empty() => {
                    let ran: Vec<String> = actions.iter().zip(results).filter(|(_, result)| result.as_deref() == Some("")).map(|(action, _)| action.to_string()).collect();
                    if !ran.is_empty() {
                        lines.push(format!("(Kith ran {})", ran.join(", ")));
                    }
                }
                What::Memory { text, state: Proposal::Saved } => lines.push(format!("(Kith remembered: {text})")),
                What::Task { task, state: TaskState::Created, .. } => lines.push(format!("(Kith created the task “{}”: {})", task.name, task.prompt)),
                What::Suggestion { text, offer_label, state, .. } => lines.push(match state {
                    SuggestionState::Open => format!("(Kith suggested: {text})"),
                    SuggestionState::Accepted => format!("(Kith suggested: {text} The user chose {offer_label}.)"),
                    SuggestionState::NotNow | SuggestionState::Never => format!("(Kith suggested: {text} The user declined.)"),
                }),
                _ => {}
            }
        }
        lines.join("\n")
    }

    /// Writes the conversation through a temporary file, only you able to read it.
    pub fn save(&mut self) {
        self.prune();
        let path = path();
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
            eprintln!("ai: saving the conversation to {}: {err}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unrun_actions_are_skipped_by_the_next_message() {
        let mut chat = Chat::default();
        chat.said("dim the screen");
        let actions = chat.push(What::Actions { actions: vec![json!({ "action": "set_brightness", "percent": 30 })], confidence: vec![0.9], results: Vec::new(), skipped: false });
        chat.said("never mind");
        assert!(matches!(chat.get_mut(actions), Some(What::Actions { skipped: true, .. })));
    }

    #[test]
    fn the_transcript_says_what_happened() {
        let mut chat = Chat::default();
        chat.said("mute and tell me a joke");
        chat.push(What::Actions { actions: vec![json!({ "action": "mute", "on": true })], confidence: vec![0.9], results: vec![Some(String::new())], skipped: false });
        chat.push(What::Answer { answer: 1, text: "A joke.".into(), files: Vec::new(), writing: false, error: String::new() });
        chat.push(What::Memory { text: "Likes jokes".into(), state: Proposal::Proposed });
        assert_eq!(chat.transcript(), "User: mute and tell me a joke\n(Kith ran {\"action\":\"mute\",\"on\":true})\nKith: A joke.");
        let json = serde_json::to_value(&chat.entries[3]).unwrap();
        assert_eq!(json["kind"], "memory");
        assert_eq!(json["state"], "proposed");
    }

    #[test]
    fn a_week_is_kept() {
        let mut chat = Chat::default();
        chat.said("old");
        chat.said("new");
        chat.entries[0].time = now() - KEPT - 60;
        assert_eq!(chat.entries().len(), 1);
    }
}
