//! Where shipped, system and personal data files meet: looks and wallpaper scenes are TOML files that Autark embeds,
//! `/usr/share/autark/<kind>/` can add to, and `~/.config/autark/<kind>/` can add to or replace.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Your folder for `kind`, then the system's.
pub fn dirs(kind: &str) -> Vec<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    config.map(|dir| dir.join("autark").join(kind)).into_iter().chain([PathBuf::from("/usr/share/autark").join(kind)]).collect()
}

/// Every file as `parse` reads it, by file name without `.toml`. The built-in ones come first, then the system's folder,
/// then yours, so a later one replaces an earlier one with the same id (`dirs` is yours first). A file that doesn't parse
/// is reported on stderr and left out.
pub fn collect<T>(builtin: &[(&str, &str)], dirs: &[PathBuf], parse: impl Fn(&str) -> Result<T, String>) -> BTreeMap<String, T> {
    let mut found = BTreeMap::new();
    for (id, text) in builtin {
        if let Ok(item) = parse(text) {
            found.insert((*id).to_owned(), item);
        }
    }
    for dir in dirs.iter().rev() {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for path in entries.flatten().map(|entry| entry.path()) {
            let (Some(id), true) = (path.file_stem().and_then(|stem| stem.to_str()), path.extension().is_some_and(|ext| ext == "toml")) else { continue };
            match std::fs::read_to_string(&path).map_err(|error| error.to_string()).and_then(|text| parse(&text)) {
                Ok(item) => {
                    found.insert(id.to_owned(), item);
                }
                Err(error) => eprintln!("{}: {error}", path.display()),
            }
        }
    }
    found
}
