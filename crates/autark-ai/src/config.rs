//! `ai.toml`: every field is optional. AI stays off until `enabled = true`.

use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Ai {
    /// The master switch. While off, only `Check`'s built-in rules answer.
    pub enabled: bool,
    /// Everything goes to Ollama, Jev's features included.
    pub local_only: bool,
    pub ollama: Ollama,
    pub features: Features,
    pub jev: Jev,
    pub mcp: Mcp,
    pub web: Web,
    pub tasks: crate::tasks::Limits,
    pub suggestions: crate::suggest::Limits,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ollama {
    pub url: String,
    /// No default: pick one after measuring it on your laptop.
    pub model: String,
    /// The model that writes Kith's answers, when a bigger and slower one is worth it there: routing, the lookup loop,
    /// ratings and tasks keep to `model`. Empty means `model` writes them too.
    pub answer_model: String,
    pub timeout_seconds: u64,
}

impl Default for Ollama {
    fn default() -> Self {
        Self { url: "http://localhost:11434".to_owned(), model: String::new(), answer_model: String::new(), timeout_seconds: 120 }
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
    /// Kith's suggestion cards.
    pub suggest: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self { route: true, rank: true, check: true, ask: true, suggest: true }
    }
}

/// Which features Jev answers, unless `local_only` is on. Off by default: text goes to the cloud only
/// when you choose. Ollama answers the rest, and whatever Jev can't.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Jev {
    /// Route: what's typed in the launcher, turned into actions.
    pub launcher: bool,
    /// Notification urgency, for Focus.
    pub notify: bool,
    /// Check: the terminal's safety check.
    pub terminal: bool,
    /// Rank, for Files' search.
    pub files: bool,
    /// Rank, for Settings' search.
    pub settings: bool,
}

/// `autark-ai mcp`. It only reads, unless you let it act.
#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default, deny_unknown_fields)]
pub struct Mcp {
    /// Offers `run_actions` to the client: the catalog's actions (volume, brightness, Wi-Fi, Focus, night light,
    /// lock, sleep, open an app), which then run without asking you. Off by default.
    pub act: bool,
}

/// Web search through Firecrawl (https://docs.firecrawl.dev), for `Search` and the MCP server's `web_search`.
/// The query goes to Firecrawl, so it is off by default, and `local_only` overrides it.
#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default, deny_unknown_fields)]
pub struct Web {
    pub search: bool,
}
