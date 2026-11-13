//! `ai.toml`: every field is optional. AI stays off until `enabled = true`. Keys of features Kith no longer has are
//! read and ignored, so an older file still loads; any other unknown key is an error.

use serde::Deserialize;
use serde::de::IgnoredAny;

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
    pub suggestions: crate::suggest::Limits,
    #[serde(rename = "mcp")]
    _mcp: IgnoredAny,
    #[serde(rename = "web")]
    _web: IgnoredAny,
    #[serde(rename = "tasks")]
    _tasks: IgnoredAny,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ollama {
    pub url: String,
    /// No default: pick one after measuring it on your laptop.
    pub model: String,
    /// The model that writes Ask's answers, when a bigger and slower one is worth it there: routing, checks and
    /// ratings keep to `model`. Empty means `model` writes them too.
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
    #[serde(rename = "rank")]
    _rank: IgnoredAny,
    pub check: bool,
    pub ask: bool,
    /// Kith's suggestion cards.
    pub suggest: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self { route: true, _rank: IgnoredAny, check: true, ask: true, suggest: true }
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
    #[serde(rename = "files")]
    _files: IgnoredAny,
    #[serde(rename = "settings")]
    _settings: IgnoredAny,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_file_still_loads() {
        let older = "enabled = true\n[features]\nrank = false\nsuggest = false\n[jev]\nfiles = true\nsettings = true\nlauncher = true\n\
                     [mcp]\nact = true\n[web]\nsearch = true\n[tasks]\nmax = 5\nminutes = 2\n";
        let ai: Ai = toml::from_str(older).unwrap();
        assert!(ai.enabled && ai.jev.launcher && !ai.features.suggest && ai.features.route);
        assert!(toml::from_str::<Ai>("enabeld = true").is_err());
    }
}
