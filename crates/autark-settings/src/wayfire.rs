//! Wayfire's own options (keyboard, shortcuts): set live over its IPC socket and written to `wayfire.ini`, so
//! they hold after a restart. The file is edited line by line; comments and order stay as they were.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde_json::Value;

/// `$WAYFIRE_CONFIG_FILE`, or `~/.config/wayfire.ini`.
pub fn ini_path() -> PathBuf {
    if let Some(path) = std::env::var_os("WAYFIRE_CONFIG_FILE") {
        return PathBuf::from(path);
    }
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("wayfire.ini")
}

/// One call on `$WAYFIRE_SOCKET` (the `ipc` plugin): a little-endian length, then JSON, each way.
pub fn call(method: &str, data: Value) -> Result<Value, String> {
    let socket = std::env::var_os("WAYFIRE_SOCKET").ok_or("Wayfire's IPC isn't on (add ipc to its plugins)")?;
    let mut stream = UnixStream::connect(&socket).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let message = serde_json::to_vec(&serde_json::json!({ "method": method, "data": data })).map_err(|error| error.to_string())?;
    stream.write_all(&(message.len() as u32).to_le_bytes()).and_then(|_| stream.write_all(&message)).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let mut length = [0; 4];
    stream.read_exact(&mut length).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let mut reply = vec![0; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut reply).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let reply: Value = serde_json::from_slice(&reply).map_err(|error| format!("Wayfire IPC: {error}"))?;
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(format!("Wayfire: {error}")),
        None => Ok(reply),
    }
}

/// An option's current value as Wayfire has it, `section/name`.
pub fn get(option: &str) -> Option<String> {
    let reply = call("wayfire/get-config-option", serde_json::json!({ "option": option })).ok()?;
    match reply.get("value")? {
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// Sets options in `wayfire.ini` and live. Wayfire rereads the file when it changes, so the live call is only
/// for speed and its failure isn't one.
pub fn set(options: &[(&str, String)]) -> Result<(), String> {
    write(options)?;
    let live: serde_json::Map<String, Value> = options.iter().map(|(option, value)| (option.to_string(), Value::String(value.clone()))).collect();
    let _ = call("wayfire/set-config-options", Value::Object(live));
    Ok(())
}

/// Writes options, `section/name`, to `wayfire.ini`.
pub fn write(options: &[(&str, String)]) -> Result<(), String> {
    let path = ini_path();
    let mut ini = Ini::parse(&std::fs::read_to_string(&path).unwrap_or_default());
    for (option, value) in options {
        let (section, key) = option.split_once('/').ok_or_else(|| format!("{option} isn't section/name"))?;
        ini.set(section, key, value);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    autark_common::files::write_atomically(&path, ini.text().as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

/// `wayfire.ini` as lines, kept as written.
pub struct Ini {
    lines: Vec<String>,
}

impl Ini {
    pub fn parse(text: &str) -> Self {
        Self { lines: text.lines().map(str::to_owned).collect() }
    }

    pub fn text(&self) -> String {
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
}
