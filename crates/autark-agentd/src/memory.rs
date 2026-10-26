//! An agent's memory: `memory.toml` in its folder, a `[[fact]]` for each. It is small on purpose, so it can sit in the
//! agent's instructions every time it answers: at most 12 facts and 2,000 characters in all. A fact that would go past
//! either is refused, and the reason says so, so nothing is quietly dropped to make room. No other agent reads it.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MAX_FACTS: usize = 12;
pub const MAX_CHARS: usize = 2000;
/// One fact may not be longer than this, so a single note can't use the whole memory.
const FACT: usize = 300;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Fact {
    pub text: String,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub added: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    fact: Vec<Fact>,
}

pub fn load(path: &Path) -> Vec<Fact> {
    std::fs::read_to_string(path).ok().and_then(|text| toml::from_str::<File>(&text).ok()).map(|file| file.fact).unwrap_or_default()
}

fn save(path: &Path, facts: Vec<Fact>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    }
    let text = toml::to_string(&File { fact: facts }).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| format!("writing {}: {err}", path.display()))
}

/// Adds a fact, or says why it can't be.
pub fn add(path: &Path, text: &str, time: u64) -> Result<(), String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("there is nothing to remember".to_owned());
    }
    if text.chars().count() > FACT {
        return Err(format!("a fact is at most {FACT} characters: say it shorter"));
    }
    let mut facts = load(path);
    if facts.iter().any(|fact| fact.text.eq_ignore_ascii_case(&text)) {
        return Ok(());
    }
    if facts.len() >= MAX_FACTS {
        return Err(format!("its memory is full ({MAX_FACTS} facts): forget one first"));
    }
    let used: usize = facts.iter().map(|fact| fact.text.chars().count()).sum();
    if used + text.chars().count() > MAX_CHARS {
        return Err(format!("its memory is full ({MAX_CHARS} characters): forget one first"));
    }
    facts.push(Fact { text, added: time });
    save(path, facts)
}

/// Forgets the `n`th fact, counting from 0.
pub fn forget(path: &Path, n: usize) -> Result<(), String> {
    let mut facts = load(path);
    if n >= facts.len() {
        return Err("there is no such fact".to_owned());
    }
    facts.remove(n);
    save(path, facts)
}

/// The facts as a part of the agent's instructions, or nothing.
pub fn as_instructions(facts: &[Fact]) -> String {
    if facts.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = facts.iter().map(|fact| format!("- {}", fact.text)).collect();
    format!("\nWhat you remember about the user (use it when it is relevant, and when asked what you remember):\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agentd-memory-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("memory.toml")
    }

    #[test]
    fn a_fact_is_kept_once_and_forgotten_by_place() {
        let p = path("keep");
        add(&p, "Prefers short answers", 5).unwrap();
        add(&p, "prefers  short answers", 6).unwrap();
        add(&p, "Lives in Cebu", 7).unwrap();
        assert_eq!(load(&p).iter().map(|f| f.text.as_str()).collect::<Vec<_>>(), ["Prefers short answers", "Lives in Cebu"]);
        forget(&p, 0).unwrap();
        assert_eq!(load(&p).len(), 1);
        assert!(forget(&p, 5).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_full_memory_refuses_and_says_why() {
        let p = path("count");
        for n in 0..MAX_FACTS {
            add(&p, &format!("fact {n}"), 1).unwrap();
        }
        let err = add(&p, "one more", 1).unwrap_err();
        assert!(err.contains("full") && err.contains("forget"));
        assert_eq!(load(&p).len(), MAX_FACTS);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn the_characters_are_capped_in_all_and_per_fact() {
        let p = path("chars");
        let long = "x".repeat(FACT);
        for _ in 0..6 {
            add(&p, &format!("{}{}", long, load(&p).len()).chars().take(FACT).collect::<String>(), 1).ok();
        }
        assert!(load(&p).iter().map(|f| f.text.chars().count()).sum::<usize>() <= MAX_CHARS);
        assert!(add(&p, &"y".repeat(FACT + 1), 1).is_err());
        assert!(add(&p, "   ", 1).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn the_instructions_list_the_facts() {
        assert_eq!(as_instructions(&[]), "");
        let facts = [Fact { text: "A".into(), added: 0 }, Fact { text: "B".into(), added: 0 }];
        assert!(as_instructions(&facts).ends_with("- A\n- B"));
    }
}
