//! `ai.toml`: every field is optional. AI stays off until `enabled = true`.

use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Ai {
    /// The master switch. While off, only `Check`'s built-in rules answer.
    pub enabled: bool,
    pub ollama: Ollama,
    pub features: Features,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ollama {
    pub url: String,
    /// No default: pick one after measuring it on your laptop.
    pub model: String,
    pub timeout_seconds: u64,
}

impl Default for Ollama {
    fn default() -> Self {
        Self { url: "http://localhost:11434".to_owned(), model: String::new(), timeout_seconds: 120 }
    }
}

/// One switch per feature, applied once `enabled` is on.
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Features {
    pub route: bool,
    pub rank: bool,
    pub check: bool,
    pub ask: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self { route: true, rank: true, check: true, ask: true }
    }
}
