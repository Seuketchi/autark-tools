//! The options menu's choices, kept in `$XDG_STATE_HOME/autark/shot.toml` along with the last area.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SaveTo {
    #[default]
    Folder,
    Clipboard,
    Both,
}

impl SaveTo {
    pub fn folder(self) -> bool {
        self != SaveTo::Clipboard
    }

    pub fn clipboard(self) -> bool {
        self != SaveTo::Folder
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Options {
    pub save_to: SaveTo,
    /// Seconds to wait before capturing: 0, 5 or 10.
    pub timer: u32,
    pub pointer: bool,
    pub thumbnail: bool,
    /// The last area chosen, as output name and logical x, y, width, height; the options bar starts there.
    pub area: Option<(String, i32, i32, i32, i32)>,
}

impl Default for Options {
    fn default() -> Self {
        Self { save_to: SaveTo::Folder, timer: 0, pointer: false, thumbnail: true, area: None }
    }
}

fn path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("autark/shot.toml")
}

impl Options {
    pub fn load() -> Self {
        std::fs::read_to_string(path()).ok().and_then(|text| toml::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = path();
        if let (Some(parent), Ok(text)) = (path.parent(), toml::to_string(self)) {
            let _ = std::fs::create_dir_all(parent);
            let _ = autark_common::files::write_atomically(&path, text.as_bytes());
        }
    }
}

/// Where screenshots go: `~/Pictures/Screenshots`, or the folder in `~/.config/autark/shot.toml`.
pub fn folder() -> PathBuf {
    #[derive(Deserialize, Default)]
    struct Config {
        folder: Option<String>,
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config: Config = autark_common::config::load("shot");
    match config.folder {
        Some(folder) if folder.starts_with("~/") => home.join(&folder[2..]),
        Some(folder) => PathBuf::from(folder),
        None => home.join("Pictures/Screenshots"),
    }
}

/// `Screenshot 2026-10-17 at 19.50.12.png`; `files::free_name` numbers it when two land in the same second.
pub fn file_name(time: &gtk::glib::DateTime) -> String {
    format!("Screenshot {}.png", time.format("%Y-%m-%d at %H.%M.%S").map(|text| text.to_string()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_and_default() {
        let options = Options { save_to: SaveTo::Both, timer: 5, pointer: true, thumbnail: false, area: Some(("eDP-1".into(), 10, 20, 300, 200)) };
        assert_eq!(toml::from_str::<Options>(&toml::to_string(&options).unwrap()).unwrap(), options);
        assert_eq!(toml::from_str::<Options>("").unwrap(), Options::default());
        assert!(SaveTo::Both.folder() && SaveTo::Both.clipboard() && !SaveTo::Clipboard.folder());
    }
}
