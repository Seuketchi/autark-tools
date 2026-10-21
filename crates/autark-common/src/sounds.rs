//! Sound sets: the sounds the tools play for an alert or a screenshot. The built-in set is made in code (see `audio::samples`);
//! a folder of `alert.wav` and `shutter.wav` in `~/.config/autark/sounds/<name>/` (or `/usr/share/autark/sounds/<name>/`) is
//! another, chosen by `set = "<name>"` in `sound.toml` or in Settings → Sound. A file that is missing or can't be read falls
//! back to the built-in sound for that one effect.

use std::path::PathBuf;

/// The set made in code.
pub const BUILT_IN: &str = "default";

/// Your folder of sets, then the system's.
pub fn dirs() -> Vec<PathBuf> {
    crate::catalog::dirs("sounds")
}

/// Every set you can choose: the built-in one, then the folders found, by name.
pub fn list_in(dirs: &[PathBuf]) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            if entry.path().is_dir()
                && let Some(name) = entry.file_name().to_str()
                && name != BUILT_IN
            {
                names.insert(name.to_owned());
            }
        }
    }
    std::iter::once(BUILT_IN.to_owned()).chain(names).collect()
}

pub fn list() -> Vec<String> {
    list_in(&dirs())
}

/// The file for `effect_name` (`alert`, `shutter`) in the set called `set`, if there is one.
pub fn file_in(dirs: &[PathBuf], set: &str, effect_name: &str) -> Option<PathBuf> {
    // A set's name is a folder name, never a path.
    if set == BUILT_IN || set.is_empty() || set.contains(['/', '\\']) || set.starts_with('.') {
        return None;
    }
    dirs.iter().map(|dir| dir.join(set).join(format!("{effect_name}.wav"))).find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_are_the_folders_found_after_the_built_in_one() {
        let dir = std::env::temp_dir().join(format!("autark-sounds-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("chime")).unwrap();
        std::fs::create_dir_all(dir.join("bell")).unwrap();
        std::fs::create_dir_all(dir.join("default")).unwrap();
        std::fs::write(dir.join("stray.wav"), "").unwrap();
        std::fs::write(dir.join("chime/alert.wav"), "").unwrap();
        let dirs = [dir.clone()];
        assert_eq!(list_in(&dirs), ["default", "bell", "chime"]);
        assert!(file_in(&dirs, "chime", "alert").is_some());
        assert!(file_in(&dirs, "chime", "shutter").is_none(), "a missing file falls back to the built-in sound");
        assert!(file_in(&dirs, "default", "alert").is_none());
        assert!(file_in(&dirs, "../chime", "alert").is_none(), "a name is not a path");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
