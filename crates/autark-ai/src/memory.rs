//! What you've asked Kith to remember, `~/.config/autark-ai/memory.toml`: one `[[fact]]` per fact, which
//! Settings shows, corrects and forgets. Read fresh for each call, so a fact forgotten there is gone here too.

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

pub fn facts() -> Vec<String> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let path = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".config")).join("autark-ai/memory.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let memory: Memory = toml::from_str(&text).unwrap_or_else(|err| {
        eprintln!("ai: {}: {err}", path.display());
        Memory::default()
    });
    memory.fact.into_iter().map(|fact| fact.text.trim().to_owned()).filter(|text| !text.is_empty()).collect()
}
