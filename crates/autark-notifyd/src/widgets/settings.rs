//! What Edit widgets changes, kept in `$XDG_STATE_HOME/autark/widgets.toml` since the config
//! files are yours to write, not ours.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Settings {
    /// Widgets switched off, by key (see `KINDS`).
    pub hidden: Vec<String>,
    /// Shows the clock and the pinned note on the desktop too.
    pub desktop: bool,
    pub weather: bool,
    /// Apps Kith never suggests quitting, by desktop ID or name.
    pub never_suggest: Vec<String>,
}

/// Every widget that can be switched off, with its name in Edit widgets.
pub const KINDS: [(&str, &str); 5] =
    [("clock", "Clock and calendar"), ("battery", "Battery"), ("resources", "Resources"), ("kith", "Kith"), ("note", "Pinned note")];

impl Settings {
    pub fn shows(&self, kind: &str) -> bool {
        !self.hidden.iter().any(|hidden| hidden == kind)
    }

    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(file()) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            eprintln!("notifyd: {}: {err}", file().display());
            Self::default()
        })
    }

    pub fn save(&self) {
        let path = file();
        let result = toml::to_string(self)
            .map_err(std::io::Error::other)
            .and_then(|text| path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text)));
        if let Err(err) = result {
            eprintln!("notifyd: saving {}: {err}", path.display());
        }
    }
}

fn file() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/state"));
    state.join("autark/widgets.toml")
}
