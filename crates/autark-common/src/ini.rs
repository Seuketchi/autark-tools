//! A small line-based editor for `wayfire.ini` and files like it. A change touches one line (or adds one at the end of its
//! section); every other byte, comment and blank line stays as it was, so a file you keep by hand survives Settings.

use std::path::{Path, PathBuf};

/// `$WAYFIRE_CONFIG_FILE`, or `wayfire.ini` in the config folder.
pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("WAYFIRE_CONFIG_FILE") {
        return PathBuf::from(path);
    }
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("wayfire.ini")
}

/// Sets `section/name` options in the file at `path` (made if it isn't there), through a temporary file renamed over it, so a
/// crash can't leave half a file. A value of `None` removes the option's line. Nothing is written when nothing changes.
pub fn update(path: &Path, options: &[(&str, Option<&str>)]) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut ini = Ini::parse(&text);
    for (option, value) in options {
        let (section, key) = option.split_once('/').ok_or_else(|| format!("{option} isn't section/name"))?;
        match value {
            Some(value) => ini.set(section, key, value),
            None => ini.remove(section, key),
        }
    }
    let updated = ini.text();
    if updated == text {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::files::write_atomically(path, updated.as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

/// `wayfire.ini` as lines, kept as written.
pub struct Ini {
    lines: Vec<String>,
    /// The text as read, which an unedited file is returned as, byte for byte.
    original: String,
}

impl Ini {
    pub fn parse(text: &str) -> Self {
        Self { lines: text.lines().map(str::to_owned).collect(), original: text.to_owned() }
    }

    pub fn text(&self) -> String {
        if self.lines.iter().map(String::as_str).eq(self.original.lines()) {
            return self.original.clone();
        }
        let mut text = self.lines.join("\n");
        text.push('\n');
        text
    }

    fn section_of(line: &str) -> Option<&str> {
        let line = line.trim();
        line.strip_prefix('[')?.strip_suffix(']').map(str::trim)
    }

    fn key_of(line: &str) -> Option<&str> {
        let line = line.trim_start();
        if line.starts_with('#') || line.starts_with(';') {
            return None;
        }
        line.split_once('=').map(|(key, _)| key.trim())
    }

    /// The keys set in a section, in order.
    pub fn keys(&self, section: &str) -> Vec<String> {
        let mut inside = false;
        let mut keys = Vec::new();
        for line in &self.lines {
            if let Some(name) = Self::section_of(line) {
                inside = name == section;
            } else if inside && let Some(key) = Self::key_of(line) {
                keys.push(key.to_owned());
            }
        }
        keys
    }

    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        let mut inside = false;
        for line in &self.lines {
            if let Some(name) = Self::section_of(line) {
                inside = name == section;
            } else if inside && Self::key_of(line) == Some(key) {
                return line.split_once('=').map(|(_, value)| value.trim().to_owned());
            }
        }
        None
    }

    /// Replaces the key's line, or adds it at the end of its section, or adds the section.
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let entry = format!("{key} = {value}");
        let mut inside = false;
        let mut last_in_section = None;
        for index in 0..self.lines.len() {
            if let Some(name) = Self::section_of(&self.lines[index]) {
                inside = name == section;
                if inside {
                    last_in_section = Some(index);
                }
                continue;
            }
            if inside {
                if Self::key_of(&self.lines[index]) == Some(key) {
                    self.lines[index] = entry;
                    return;
                }
                if !self.lines[index].trim().is_empty() {
                    last_in_section = Some(index);
                }
            }
        }
        match last_in_section {
            Some(index) => self.lines.insert(index + 1, entry),
            None => {
                if self.lines.last().is_some_and(|line| !line.trim().is_empty()) {
                    self.lines.push(String::new());
                }
                self.lines.push(format!("[{section}]"));
                self.lines.push(entry);
            }
        }
    }

    /// Removes the key's line, if it has one.
    pub fn remove(&mut self, section: &str, key: &str) {
        let mut inside = false;
        let found = (0..self.lines.len()).find(|&index| {
            if let Some(name) = Self::section_of(&self.lines[index]) {
                inside = name == section;
                return false;
            }
            inside && Self::key_of(&self.lines[index]) == Some(key)
        });
        if let Some(index) = found {
            self.lines.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_comments_and_order() {
        let mut ini = Ini::parse("# mine\n[core]\nplugins = ipc\n\n[input]\n# fast\nkb_repeat_rate = 40\n\n[command]\nbinding_x = <super> KEY_X\n");
        assert_eq!(ini.get("input", "kb_repeat_rate").as_deref(), Some("40"));
        ini.set("input", "kb_repeat_rate", "30");
        ini.set("input", "xkb_layout", "us,ph");
        ini.set("autostart", "nightlight", "autark-nightlight daemon");
        assert_eq!(
            ini.text(),
            "# mine\n[core]\nplugins = ipc\n\n[input]\n# fast\nkb_repeat_rate = 30\nxkb_layout = us,ph\n\n[command]\nbinding_x = <super> KEY_X\n\n[autostart]\nnightlight = autark-nightlight daemon\n"
        );
        assert_eq!(ini.get("core", "kb_repeat_rate"), None);
    }

    #[test]
    fn an_untouched_file_comes_back_byte_for_byte() {
        for text in ["", "[a]\nx = 1", "[a]\nx = 1\n", "# c\n\n\n[a]\n  y=2  \n\n", "no section = here\r\n"] {
            assert_eq!(Ini::parse(text).text(), text);
        }
        // Setting a value to what it is changes the line's spacing only if it differs; the rest stays.
        let mut ini = Ini::parse("[a]\nx = 1\n\n[b]\ny = 2\n");
        ini.set("a", "x", "1");
        assert_eq!(ini.text(), "[a]\nx = 1\n\n[b]\ny = 2\n");
    }

    #[test]
    fn a_key_is_removed_from_its_own_section_only() {
        let mut ini = Ini::parse("[a]\nx = 1\ny = 2\n[b]\nx = 3\n");
        ini.remove("b", "x");
        ini.remove("a", "nothing");
        ini.remove("c", "x");
        assert_eq!(ini.text(), "[a]\nx = 1\ny = 2\n[b]\n");
    }

    #[test]
    fn update_writes_through_a_file_and_only_when_something_changes() {
        let dir = std::env::temp_dir().join(format!("autark-ini-{}", std::process::id()));
        let file = dir.join("sub/wayfire.ini");
        update(&file, &[("core/plugins", Some("ipc"))]).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[core]\nplugins = ipc\n");
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        update(&file, &[("core/plugins", Some("ipc"))]).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before);
        update(&file, &[("core/plugins", None), ("input/kb_repeat_rate", Some("30"))]).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[core]\n\n[input]\nkb_repeat_rate = 30\n");
        assert!(update(&file, &[("nosection", Some("x"))]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
