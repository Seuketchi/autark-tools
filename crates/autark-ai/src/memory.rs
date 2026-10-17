//! What Kith remembers about you, `~/.config/autark-ai/memory.toml`: one `[[fact]]` per fact, each one you
//! saved, which Settings shows, corrects and forgets. Read fresh for each call, so a fact forgotten there is
//! gone here too.

use std::io::Write;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize, Default)]
struct Memory {
    #[serde(default)]
    fact: Vec<Fact>,
}

#[derive(Deserialize)]
struct Fact {
    #[serde(default)]
    text: String,
}

fn path() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("autark-ai/memory.toml")
}

pub fn facts() -> Vec<String> {
    let path = path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let memory: Memory = toml::from_str(&text).unwrap_or_else(|err| {
        eprintln!("ai: {}: {err}", path.display());
        Memory::default()
    });
    memory.fact.into_iter().map(|fact| fact.text.trim().to_owned()).filter(|text| !text.is_empty()).collect()
}

/// Adds a fact you saved, at the end of the file, so its comments and the facts before it stay as they are.
pub fn add(text: &str) -> Result<(), String> {
    add_to(&path(), text)
}

fn add_to(path: &std::path::Path, text: &str) -> Result<(), String> {
    let failed = |err: std::io::Error| format!("{}: {err}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(failed)?;
    }
    let before = std::fs::read_to_string(path).unwrap_or_default();
    let gap = if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let entry = format!("{gap}[[fact]]\ntext = {}\nadded = {}\n", toml::Value::String(text.to_owned()), chrono::Local::now().format("%Y-%m-%d"));
    std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut file| file.write_all(entry.as_bytes())).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_fact_is_added_after_the_rest() {
        let dir = std::env::temp_dir().join(format!("autark-ai-memory-{}", std::process::id()));
        let path = dir.join("memory.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "# mine\n[[fact]]\ntext = \"Uses Autark\"\nadded = 2026-10-16").unwrap();
        add_to(&path, "Says \"hi\" a lot").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(text.starts_with("# mine\n[[fact]]\ntext = \"Uses Autark\"\nadded = 2026-10-16\n\n[[fact]]\ntext = 'Says \"hi\" a lot'\nadded = "), "{text}");
        let memory: Memory = toml::from_str(&text).unwrap();
        assert_eq!(memory.fact.len(), 2);
    }
}
