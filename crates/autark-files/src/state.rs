//! What the app remembers between runs, in `$XDG_STATE_HOME/autark/files.toml`: favourites,
//! the view and whether hidden files show.

use std::path::PathBuf;

use gtk::glib;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct State {
    /// Folders in the sidebar, `~/` for the home folder.
    pub favourites: Vec<String>,
    pub list_view: bool,
    pub show_hidden: bool,
}

impl Default for State {
    fn default() -> Self {
        let mut favourites = vec!["~".to_owned()];
        for dir in [glib::UserDirectory::Desktop, glib::UserDirectory::Documents, glib::UserDirectory::Downloads] {
            if let Some(path) = glib::user_special_dir(dir).filter(|path| path.is_dir() && *path != glib::home_dir()) {
                favourites.push(crate::entry::tilde(&path));
            }
        }
        Self { favourites, list_view: false, show_hidden: false }
    }
}

pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) => glib::home_dir().join(rest.trim_start_matches('/')),
        None => PathBuf::from(path),
    }
}

fn file() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join("autark/files.toml")
}

impl State {
    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(file()) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            eprintln!("files: {}: {err}", file().display());
            Self::default()
        })
    }

    pub fn save(&self) {
        let path = file();
        let result = toml::to_string(self)
            .map_err(std::io::Error::other)
            .and_then(|text| path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text)));
        if let Err(err) = result {
            eprintln!("files: saving {}: {err}", path.display());
        }
    }
}
