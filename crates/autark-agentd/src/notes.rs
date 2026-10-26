//! Adding to a note: "add milk to my grocery list". Code reads the sentence, as with `remember`, and the agent proposes
//! the line for you to add or not; nothing is written until you say so. What is written is only ever lines added at
//! the end of a Markdown note in your Notes vault (a note is made if there is none), never a change to what is there,
//! never outside the vault, and never in a note named for a secret.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The most items one sentence adds, and the longest one.
const ITEMS: usize = 10;
const ITEM: usize = 200;
/// A note this big isn't added to.
const BIGGEST: u64 = 256 * 1024;
/// Notes named for these are not touched, as Kith's own reading of notes skips them.
const SECRETS: [&str; 6] = ["password", "secret", "token", "credential", "private key", "recovery"];
const ASKS: [&str; 5] = ["can you please ", "could you please ", "can you ", "could you ", "please "];
const VERBS: [&str; 4] = ["add ", "put ", "write down ", "write "];
const JOINERS: [&str; 6] = [" to my ", " to the ", " on my ", " on the ", " in my ", " in the "];

/// A request to add to a note.
#[derive(Debug, PartialEq)]
pub struct Add {
    /// The note's name, as its file would be named without `.md`.
    pub note: String,
    pub items: Vec<String>,
}

/// The request in `said`, if it is one: items, then "to my … list" or "in my … note".
pub fn parse(said: &str) -> Option<Add> {
    let said = said.trim().trim_end_matches(['.', '!']).trim();
    // ASCII-only lowercasing keeps every offset the same in both.
    let lower = said.to_ascii_lowercase();
    let at = ASKS.iter().find(|ask| lower.starts_with(*ask)).map_or(0, |ask| ask.len());
    let verb = VERBS.iter().find(|verb| lower[at..].starts_with(*verb))?;
    let body = at + verb.len();
    let (joiner_at, joiner) = JOINERS.iter().filter_map(|joiner| lower[body..].rfind(joiner).map(|found| (body + found, *joiner))).max_by_key(|(found, _)| *found)?;
    let items = items(&said[body..joiner_at])?;
    let note = name(&said[joiner_at + joiner.len()..])?;
    Some(Add { note, items })
}

/// "milk, eggs and bread" as three items.
fn items(text: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    for part in text.split(',') {
        let lower = part.to_ascii_lowercase();
        let mut start = 0;
        let mut pieces = Vec::new();
        while let Some(found) = lower[start..].find(" and ") {
            pieces.push(&part[start..start + found]);
            start += found + " and ".len();
        }
        pieces.push(&part[start..]);
        for piece in pieces {
            let item = piece.split_whitespace().collect::<Vec<_>>().join(" ");
            let item = item.strip_prefix("and ").unwrap_or(&item).trim().to_owned();
            if !item.is_empty() {
                items.push(item);
            }
        }
    }
    (!items.is_empty() && items.len() <= ITEMS && items.iter().all(|item| item.chars().count() <= ITEM)).then_some(items)
}

/// A note's name from "grocery list" or "work notes": a list keeps its word, a note loses it. Only a list, a note or
/// notes, so that "to my calendar" isn't taken for one.
fn name(phrase: &str) -> Option<String> {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    let (last, rest) = words.split_last()?;
    let last = last.to_ascii_lowercase();
    let kept: Vec<&str> = match last.as_str() {
        "list" => words.clone(),
        "note" | "notes" => rest.to_vec(),
        _ => return None,
    };
    let name = kept.join(" ");
    let fine = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'' | '_');
    if name.is_empty() || kept.len() > 4 || name.chars().count() > 40 || !name.chars().all(fine) || name.starts_with(['-', '.', '\'']) {
        return None;
    }
    let mut chars = name.chars();
    let first = chars.next()?.to_uppercase().to_string();
    Some(format!("{first}{}", chars.as_str()))
}

/// The vault: Notes' own setting, else the folder it last opened, else `~/Notes`. The same as Notes and Kith choose it.
pub fn vault() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/state"));
    vault_in(&home, &config, &state)
}

fn vault_in(home: &Path, config: &Path, state: &Path) -> PathBuf {
    let chosen = std::fs::read_to_string(config.join("autark/notes.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|value| value.get("vault").and_then(|vault| vault.as_str()).map(str::to_owned))
        .map(|vault| vault.strip_prefix("~/").map_or_else(|| PathBuf::from(&vault), |rest| home.join(rest)))
        .or_else(|| std::fs::read_to_string(state.join("autark/notes-vault")).ok().map(|text| PathBuf::from(text.trim())));
    chosen.filter(|path| path.is_dir()).unwrap_or_else(|| home.join("Notes"))
}

/// The note called `name` in `vault`: one that is there (its name matched without regard to case, in the vault or a
/// folder up to three down, dot-folders skipped), else where a new one would go.
fn find(vault: &Path, name: &str) -> PathBuf {
    let mut stack = vec![(vault.to_owned(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() && depth < 3 {
                stack.push((entry.path(), depth + 1));
            } else if kind.is_file() && file_name.strip_suffix(".md").is_some_and(|stem| stem.eq_ignore_ascii_case(name)) {
                return entry.path();
            }
        }
    }
    vault.join(format!("{name}.md"))
}

/// Adds the items to the end of the note as unticked tasks, and says which note it was.
pub fn append(vault: &Path, add: &Add) -> Result<PathBuf, String> {
    let lower = add.note.to_lowercase();
    if SECRETS.iter().any(|secret| lower.contains(secret)) {
        return Err("a note named for a secret isn't added to".to_owned());
    }
    if !vault.is_dir() {
        return Err(format!("there is no Notes folder at {}", vault.display()));
    }
    let path = find(vault, &add.note);
    let existing = std::fs::metadata(&path).ok();
    if existing.as_ref().is_some_and(|meta| meta.len() > BIGGEST) {
        return Err(format!("{} is too big to add to", add.note));
    }
    // A note that doesn't end its last line gets one first.
    let needs_newline = existing.as_ref().is_some_and(|meta| meta.len() > 0) && !std::fs::read(&path).map_err(|err| format!("reading {}: {err}", path.display()))?.ends_with(b"\n");
    let mut text = if needs_newline { "\n".to_owned() } else { String::new() };
    for item in &add.items {
        text.push_str(&format!("- [ ] {item}\n"));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path).map_err(|err| format!("opening {}: {err}", path.display()))?;
    file.write_all(text.as_bytes()).map_err(|err| format!("writing {}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentd-notes-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn add(note: &str, items: &[&str]) -> Add {
        Add { note: note.to_owned(), items: items.iter().map(|item| (*item).to_owned()).collect() }
    }

    #[test]
    fn a_request_to_add_gives_the_items_and_the_note() {
        assert_eq!(parse("add milk to my grocery list"), Some(add("Grocery list", &["milk"])));
        assert_eq!(parse("Please add milk, eggs and bread to my grocery list."), Some(add("Grocery list", &["milk", "eggs", "bread"])));
        assert_eq!(parse("put batteries on the shopping list"), Some(add("Shopping list", &["batteries"])));
        assert_eq!(parse("could you add call Ana to my work notes"), Some(add("Work", &["call Ana"])));
        // The last "to my" is the note's; an item may say "to".
        assert_eq!(parse("add talk to my boss to my work note"), Some(add("Work", &["talk to my boss"])));
    }

    #[test]
    fn other_sentences_are_not_requests_to_add() {
        assert_eq!(parse("add a meeting to my calendar"), None);
        assert_eq!(parse("add milk"), None);
        assert_eq!(parse("what should I add to my grocery list?"), None);
        assert_eq!(parse("add milk to my list of things"), None);
        assert_eq!(parse("add  to my grocery list"), None);
        assert_eq!(parse("add milk to my ../etc list"), None);
        assert_eq!(parse("add milk to my a/b list"), None);
        assert_eq!(parse("add milk to my .hidden list"), None);
        assert_eq!(parse("add milk to my notes"), None);
    }

    #[test]
    fn too_many_or_too_long_items_are_refused() {
        assert_eq!(parse(&format!("add {} to my grocery list", ["x"; 11].join(", "))), None);
        assert_eq!(parse(&format!("add {} to my grocery list", "y".repeat(ITEM + 1))), None);
    }

    #[test]
    fn items_are_lines_that_cannot_carry_markdown_of_their_own() {
        assert_eq!(parse("add milk\n# Heading\n- [ ] x to my grocery list"), Some(add("Grocery list", &["milk # Heading - [ ] x"])));
    }

    #[test]
    fn a_note_is_made_and_then_added_to_at_its_end() {
        let vault = folder("append");
        let path = append(&vault, &add("Grocery list", &["milk", "eggs"])).unwrap();
        assert_eq!(path, vault.join("Grocery list.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] milk\n- [ ] eggs\n");
        append(&vault, &add("Grocery list", &["bread"])).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] milk\n- [ ] eggs\n- [ ] bread\n");
        let _ = std::fs::remove_dir_all(vault);
    }

    #[test]
    fn what_is_there_is_kept_and_a_missing_last_newline_is_added() {
        let vault = folder("keep");
        std::fs::write(vault.join("Todo list.md"), "# Todo\n- [x] done").unwrap();
        append(&vault, &add("Todo list", &["next"])).unwrap();
        assert_eq!(std::fs::read_to_string(vault.join("Todo list.md")).unwrap(), "# Todo\n- [x] done\n- [ ] next\n");
        let _ = std::fs::remove_dir_all(vault);
    }

    #[test]
    fn an_existing_note_is_found_in_a_folder_without_regard_to_case() {
        let vault = folder("find");
        std::fs::create_dir_all(vault.join("home/lists")).unwrap();
        std::fs::write(vault.join("home/lists/grocery LIST.md"), "").unwrap();
        std::fs::create_dir_all(vault.join(".git")).unwrap();
        std::fs::write(vault.join(".git/Grocery list.md"), "").unwrap();
        let path = append(&vault, &add("Grocery list", &["milk"])).unwrap();
        assert_eq!(path, vault.join("home/lists/grocery LIST.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] milk\n");
        let _ = std::fs::remove_dir_all(vault);
    }

    #[test]
    fn a_secret_named_a_big_or_a_missing_vault_is_refused() {
        let vault = folder("refuse");
        assert!(append(&vault, &add("Password list", &["x"])).is_err());
        std::fs::write(vault.join("Big list.md"), vec![b'x'; BIGGEST as usize + 1]).unwrap();
        assert!(append(&vault, &add("Big list", &["x"])).unwrap_err().contains("too big"));
        assert!(append(&vault.join("nowhere"), &add("A list", &["x"])).is_err());
        assert!(!vault.join("Password list.md").exists());
        let _ = std::fs::remove_dir_all(vault);
    }

    #[test]
    fn the_vault_is_notes_own_choice_else_its_last_else_the_notes_folder() {
        let base = folder("vault");
        let (home, config, state) = (base.join("home"), base.join("config"), base.join("state"));
        std::fs::create_dir_all(home.join("Notes")).unwrap();
        std::fs::create_dir_all(home.join("Vault")).unwrap();
        assert_eq!(vault_in(&home, &config, &state), home.join("Notes"));
        std::fs::create_dir_all(config.join("autark")).unwrap();
        std::fs::write(config.join("autark/notes.toml"), "vault = \"~/Vault\"\n").unwrap();
        assert_eq!(vault_in(&home, &config, &state), home.join("Vault"));
        let _ = std::fs::remove_dir_all(base);
    }
}
