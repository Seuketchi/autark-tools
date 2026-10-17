//! Config files: `$XDG_CONFIG_HOME/autark/<name>.toml` overrides `/etc/autark/<name>.toml`.

use std::path::PathBuf;

use serde::de::DeserializeOwned;

/// Loads `<name>.toml` from the user's config, then the system's, falling back to defaults.
/// A file that fails to parse is reported and skipped, so a typo never stops the desktop starting.
pub fn load<T: DeserializeOwned + Default>(name: &str) -> T {
    for path in paths(name) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match toml::from_str(&text) {
            Ok(config) => return config,
            Err(err) => eprintln!("{}: {err}", path.display()),
        }
    }
    T::default()
}

fn paths(name: &str) -> Vec<PathBuf> {
    let file = format!("{name}.toml");
    let user = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    user.map(|dir| dir.join("autark").join(&file))
        .into_iter()
        .chain([PathBuf::from("/etc/autark").join(&file)])
        .collect()
}
