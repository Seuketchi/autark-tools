//! The files Kith may read for an answer: ones you name in what you type, or that a memory points to, and only
//! in your home folder, never a dotfile or anything in a dot-folder (keys, tokens and browser profiles live
//! there). Each file read is shown to you, and a long one is cut to its end, where a log's news is.

use std::path::{Component, Path, PathBuf};

/// The most read from one file, and from all of them, so the question fits a small model's context.
const PER_FILE: usize = 8 * 1024;
const IN_ALL: usize = 16 * 1024;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// The paths written in `text`, as `~/…`: words starting with `~/` or `/`, or with a `/` inside, such as
/// `autark/log.md` for one in your home folder that's there. Kept in the order written, each once.
pub fn named(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for word in text.split_whitespace() {
        let word = word.trim_matches(|c: char| "\"'`“”‘’()[]{}<>,;:!?".contains(c)).trim_end_matches('.');
        if !word.contains('/') || word.contains("://") || word.len() < 3 {
            continue;
        }
        let Ok(path) = allowed(word) else { continue };
        // "and/or" isn't a path; a bare one counts only if it's there.
        if !word.starts_with(['~', '/']) && !path.exists() {
            continue;
        }
        let shown = shown(&path);
        if !found.contains(&shown) {
            found.push(shown);
        }
    }
    found
}

/// `path` resolved under home if Kith may read it, else why not. The check runs on the path as written and
/// again once symlinks are followed, so a link can't lead out.
pub fn allowed(path: &str) -> Result<PathBuf, String> {
    let home = home();
    let path = match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path.starts_with('/') => PathBuf::from(path),
        None => home.join(path),
    };
    inside(&path, &home)?;
    if let (Ok(real), Ok(real_home)) = (path.canonicalize(), home.canonicalize()) {
        inside(&real, &real_home)?;
    }
    Ok(path)
}

fn inside(path: &Path, home: &Path) -> Result<(), String> {
    let rest = path.strip_prefix(home).map_err(|_| "it isn't in your home folder".to_owned())?;
    for part in rest.components() {
        match part {
            Component::Normal(name) if name.to_string_lossy().starts_with('.') => return Err("Kith doesn't read dotfiles or dot-folders".to_owned()),
            Component::Normal(_) => {}
            _ => return Err("it isn't in your home folder".to_owned()),
        }
    }
    if rest.as_os_str().is_empty() {
        return Err("that's your home folder, not a file".to_owned());
    }
    Ok(())
}

/// `path` under home written as `~/…`.
pub fn shown(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Reads each file Kith may read, in turn, for the model's context, and says how each went: an empty
/// reason for read, else why not.
pub fn read(paths: &[String]) -> (String, Vec<(String, String)>) {
    let mut context = String::new();
    let mut outcome = Vec::new();
    for path in paths {
        let left = IN_ALL.saturating_sub(context.len());
        let result = allowed(path).and_then(|resolved| {
            if left < 256 {
                return Err("there's no room left for it in this question".to_owned());
            }
            one(&resolved, left.min(PER_FILE))
        });
        match result {
            Ok((text, note)) => {
                context.push_str(&format!("File {path}{note}:\n{text}\n\n"));
                outcome.push((path.clone(), String::new()));
            }
            Err(reason) => outcome.push((path.clone(), reason)),
        }
    }
    (context, outcome)
}

/// The file's text, or its last `limit` bytes with a note saying so.
fn one(path: &Path, limit: usize) -> Result<(String, String), String> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "there's no such file".to_owned(),
        _ => error.to_string(),
    })?;
    if !metadata.is_file() {
        return Err("it isn't a file".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes.contains(&0) {
        return Err("it isn't text".to_owned());
    }
    if bytes.len() <= limit {
        return Ok((String::from_utf8_lossy(&bytes).into_owned(), String::new()));
    }
    let mut start = bytes.len() - limit;
    // From the next line, so the first line read is whole.
    if let Some(newline) = bytes[start..].iter().position(|byte| *byte == b'\n') {
        start += newline + 1;
    }
    let note = format!(" (its last {} KB of {} KB)", (bytes.len() - start).div_ceil(1024), bytes.len().div_ceil(1024));
    Ok((String::from_utf8_lossy(&bytes[start..]).into_owned(), note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_home_and_never_dot_paths() {
        let home = home();
        assert_eq!(allowed("~/autark/log.md").unwrap(), home.join("autark/log.md"));
        assert_eq!(allowed("notes/todo.md").unwrap(), home.join("notes/todo.md"));
        assert!(allowed("~/.ssh/id_ed25519").is_err());
        assert!(allowed("~/projects/.env").is_err());
        assert!(allowed("~/autark/../.config/x").is_err());
        assert!(allowed("/etc/shadow").is_err());
        assert!(allowed("~/").is_err());
    }

    #[test]
    fn paths_are_found_in_a_sentence() {
        let text = "why did ~/autark/log.md stop? see “~/notes/build.txt”, https://example.com/x, and/or ~/.ssh/config.";
        assert_eq!(named(text), vec!["~/autark/log.md", "~/notes/build.txt"]);
    }
}
