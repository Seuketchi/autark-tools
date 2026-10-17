//! Pins and the open note, kept in `.autark-notes/state.toml` inside the vault so the Markdown
//! stays clean.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct State {
    /// Note paths, relative to the vault.
    pub pinned: Vec<String>,
    pub open: Option<String>,
}

fn file(root: &Path) -> PathBuf {
    root.join(".autark-notes/state.toml")
}

impl State {
    pub fn load(root: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(file(root)) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            eprintln!("notes: {}: {err}", file(root).display());
            Self::default()
        })
    }

    pub fn save(&self, root: &Path) {
        let path = file(root);
        let result = toml::to_string(self)
            .map_err(std::io::Error::other)
            .and_then(|text| path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text)));
        if let Err(err) = result {
            eprintln!("notes: saving {}: {err}", path.display());
        }
    }

    pub fn is_pinned(&self, path: &str) -> bool {
        self.pinned.iter().any(|pinned| pinned == path)
    }
}
