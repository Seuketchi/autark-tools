//! An agent's memory: `memory.toml` in its folder, a `[[fact]]` for each. It is small on purpose, so it can sit in the
//! agent's instructions every time it answers: at most 12 facts and 2,000 characters in all. A fact that would go past
//! either is refused, and the reason says so, so nothing is quietly dropped to make room. No other agent reads it.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MAX_FACTS: usize = 12;
pub const MAX_CHARS: usize = 2000;
/// The version of `memory.toml` this writes.
const VERSION: u32 = 1;
/// One fact may not be longer than this, so a single note can't use the whole memory.
const FACT: usize = 300;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Fact {
    pub text: String,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub added: u64,
}

#[derive(Serialize, Deserialize)]
struct File {
    /// Missing in the first files, which are version 1.
    #[serde(default = "first")]
    version: u32,
    #[serde(default)]
    fact: Vec<Fact>,
}

fn first() -> u32 {
    1
}

/// The facts kept at `path`, none when it hasn't been made. A file that can't be read or understood is an error, so it
/// is never taken for an empty memory and written over.
pub fn load(path: &Path) -> Result<Vec<Fact>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("its memory.toml can't be read: {err}")),
    };
    let file = toml::from_str::<File>(&text).map_err(|err| format!("its memory.toml can't be understood, so it is left as it is: {err}"))?;
    if file.version > VERSION {
        return Err(format!("its memory.toml is from a newer version (format {}), so it is left as it is", file.version));
    }
    Ok(file.fact)
}

fn save(path: &Path, facts: Vec<Fact>) -> Result<(), String> {
    let text = toml::to_string(&File { version: VERSION, fact: facts }).map_err(|err| err.to_string())?;
    crate::files::write(path, text.as_bytes()).map_err(|err| format!("writing {}: {err}", path.display()))
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
    let mut facts = load(path)?;
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

/// Forgets the fact with these words (spacing and case aside, as `add` tells them apart).
pub fn forget(path: &Path, text: &str) -> Result<(), String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut facts = load(path)?;
    let Some(n) = facts.iter().position(|fact| fact.text.split_whitespace().collect::<Vec<_>>().join(" ").eq_ignore_ascii_case(&text)) else {
        return Err("there is no such fact".to_owned());
    };
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
        let dir = std::env::temp_dir().join(format!("alfred-memory-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("memory.toml")
    }

    #[test]
    fn a_fact_is_kept_once_and_forgotten_by_its_words() {
        let p = path("keep");
        add(&p, "Prefers short answers", 5).unwrap();
        add(&p, "prefers  short answers", 6).unwrap();
        add(&p, "Lives in Cebu", 7).unwrap();
        assert_eq!(load(&p).unwrap().iter().map(|f| f.text.as_str()).collect::<Vec<_>>(), ["Prefers short answers", "Lives in Cebu"]);
        // A list read before another fact went is no help: the words name the fact, wherever it now is.
        forget(&p, "lives in  cebu").unwrap();
        assert_eq!(load(&p).unwrap().iter().map(|f| f.text.as_str()).collect::<Vec<_>>(), ["Prefers short answers"]);
        assert!(forget(&p, "Lives in Cebu").is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_memory_that_cant_be_read_is_never_written_over() {
        let p = path("unreadable");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "[[fact]]\ntext = \"Lives in Cebu\"\nnot toml at all").unwrap();
        assert!(load(&p).is_err());
        assert!(add(&p, "Prefers short answers", 1).unwrap_err().contains("memory.toml"));
        assert!(forget(&p, "Lives in Cebu").is_err());
        assert!(std::fs::read_to_string(&p).unwrap().contains("not toml at all"));
        // A memory not made yet is an empty one.
        assert_eq!(load(&path("none")).unwrap(), []);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_memory_says_its_version_and_one_from_a_newer_version_is_left_alone() {
        let p = path("version");
        add(&p, "Lives in Cebu", 1).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().starts_with(&format!("version = {VERSION}\n")));
        let newer = format!("version = {}\n\n[[fact]]\ntext = \"Lives in Cebu\"\n", VERSION + 1);
        std::fs::write(&p, &newer).unwrap();
        assert!(load(&p).is_err());
        assert!(add(&p, "Prefers short answers", 1).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), newer);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn the_memory_file_and_its_folder_are_only_yours() {
        use std::os::unix::fs::PermissionsExt;
        let p = path("mode");
        add(&p, "Lives in Cebu", 1).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(p.parent().unwrap()), mode(&p)), (0o700, 0o600));
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
        assert_eq!(load(&p).unwrap().len(), MAX_FACTS);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn the_characters_are_capped_in_all_and_per_fact() {
        let p = path("chars");
        let long = "x".repeat(FACT);
        for _ in 0..6 {
            add(&p, &format!("{}{}", long, load(&p).unwrap().len()).chars().take(FACT).collect::<String>(), 1).ok();
        }
        assert!(load(&p).unwrap().iter().map(|f| f.text.chars().count()).sum::<usize>() <= MAX_CHARS);
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
