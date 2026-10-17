//! `notes.toml`: every field is optional.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Notes {
    /// The vault to open; `~/` means your home folder. Without it, the last one opened, then
    /// `~/Notes`.
    pub vault: Option<String>,
}

pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where the last opened vault is remembered.
fn last_vault_file() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"));
    state.join("autark/notes-vault")
}

pub fn last_vault() -> Option<PathBuf> {
    let text = std::fs::read_to_string(last_vault_file()).ok()?;
    Some(PathBuf::from(text.trim())).filter(|path| path.is_dir())
}

pub fn remember_vault(root: &std::path::Path) {
    let file = last_vault_file();
    let result = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, root.to_string_lossy().as_bytes()));
    if let Err(err) = result {
        eprintln!("notes: remembering the vault: {err}");
    }
}
