//! What the assistant remembers about you, `~/.config/autark-ai/memory.toml`: one `[[fact]]` per fact, its
//! `text` and the date it was `added`. The assistant proposes each one and you approve it; Settings shows,
//! corrects and forgets them, keeping the file's comments.
//!
//! ```toml
//! [[fact]]
//! text = "Works on Autark in the evenings"
//! added = 2026-10-16
//! ```

use std::path::PathBuf;

use toml_edit::{DocumentMut, Item};

#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub text: String,
    /// As written: `2026-10-16`.
    pub added: Option<String>,
}

pub fn path() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("autark-ai/memory.toml")
}

pub fn read() -> Vec<Fact> {
    facts(&crate::config::read_path(&path()))
}

fn facts(document: &DocumentMut) -> Vec<Fact> {
    let Some(facts) = document.get("fact").and_then(Item::as_array_of_tables) else { return Vec::new() };
    facts
        .iter()
        .filter_map(|fact| {
            let text = fact.get("text")?.as_str()?.trim().to_owned();
            let added = fact.get("added").and_then(|added| added.as_datetime().map(|date| date.to_string()).or_else(|| added.as_str().map(str::to_owned)));
            (!text.is_empty()).then_some(Fact { text, added })
        })
        .collect()
}

/// Changes the file, if `fact` is still the `index`th one: the assistant may have changed it meanwhile.
fn change(index: usize, fact: &Fact, apply: impl FnOnce(&mut DocumentMut, usize)) -> Result<(), String> {
    let path = path();
    let mut document = crate::config::read_path(&path);
    let at = locate(&document, index, fact).ok_or_else(stale)?;
    apply(&mut document, at);
    crate::config::write_path(&path, &document)
}

/// Where the `index`th fact is among all the `[[fact]]` tables, counting ones without text that `facts`
/// skips, if it's still `fact`.
fn locate(document: &DocumentMut, index: usize, fact: &Fact) -> Option<usize> {
    if facts(document).get(index) != Some(fact) {
        return None;
    }
    let tables = document.get("fact")?.as_array_of_tables()?;
    tables.iter().enumerate().filter(|(_, table)| table.get("text").and_then(Item::as_str).is_some_and(|text| !text.trim().is_empty())).map(|(at, _)| at).nth(index)
}

fn stale() -> String {
    "memory.toml changed while it was open here; it’s shown afresh now.".to_owned()
}

pub fn correct(index: usize, fact: &Fact, text: &str) -> Result<(), String> {
    change(index, fact, |document, at| {
        if let Some(table) = document.get_mut("fact").and_then(Item::as_array_of_tables_mut).and_then(|facts| facts.get_mut(at)) {
            match table.get_mut("text").and_then(Item::as_value_mut) {
                // Keep the comment after the old text.
                Some(old) => {
                    let decor = old.decor().clone();
                    *old = text.trim().into();
                    *old.decor_mut() = decor;
                }
                None => {
                    table.insert("text", toml_edit::value(text.trim()));
                }
            }
        }
    })
}

pub fn forget(index: usize, fact: &Fact) -> Result<(), String> {
    change(index, fact, remove)
}

/// Forgets every fact, keeping the file's comments and anything else in it.
pub fn forget_all() -> Result<(), String> {
    let path = path();
    let mut document = crate::config::read_path(&path);
    while document.get("fact").and_then(Item::as_array_of_tables).is_some_and(|facts| !facts.is_empty()) {
        remove(&mut document, 0);
    }
    document.remove("fact");
    crate::config::write_path(&path, &document)
}

/// Removes the `at`th `[[fact]]` table. The comments written above it (the file's own, for the first one)
/// stay, above the next fact or at the end.
fn remove(document: &mut DocumentMut, at: usize) {
    let Some(facts) = document.get_mut("fact").and_then(Item::as_array_of_tables_mut) else { return };
    let above = facts.get(at).and_then(|table| table.decor().prefix()).and_then(|prefix| prefix.as_str()).map(comments).unwrap_or_default();
    facts.remove(at);
    if above.is_empty() {
        return;
    }
    match facts.get_mut(at) {
        Some(next) => {
            let own = next.decor().prefix().and_then(|prefix| prefix.as_str()).unwrap_or_default().to_owned();
            next.decor_mut().set_prefix(format!("{above}{own}"));
        }
        None => {
            let trailing = document.trailing().as_str().unwrap_or_default().to_owned();
            document.set_trailing(format!("{above}{trailing}"));
        }
    }
}

/// The comment lines in a table's prefix, blank lines between them dropped.
fn comments(prefix: &str) -> String {
    prefix.lines().filter(|line| line.trim_start().starts_with('#')).map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_are_read_in_order_skipping_empty_ones() {
        let document: DocumentMut = "# mine\n[[fact]]\ntext = \"One\"\nadded = 2026-10-16\n\n[[fact]]\ntext = \"\"\n\n[[fact]]\ntext = \"Two\"\n".parse().unwrap();
        let two = Fact { text: "Two".into(), added: None };
        assert_eq!(facts(&document), vec![Fact { text: "One".into(), added: Some("2026-10-16".into()) }, two.clone()]);
        assert_eq!(locate(&document, 1, &two), Some(2));
        assert_eq!(locate(&document, 0, &two), None);
    }

    #[test]
    fn forgetting_keeps_the_comments_above() {
        let mut document: DocumentMut = "# mine\n[[fact]]\ntext = \"One\"\n\n# second\n[[fact]]\ntext = \"Two\"\n".parse().unwrap();
        remove(&mut document, 0);
        assert_eq!(document.to_string(), "# mine\n\n# second\n[[fact]]\ntext = \"Two\"\n");
        remove(&mut document, 0);
        document.remove("fact");
        assert_eq!(document.to_string(), "# mine\n# second\n");
    }
}
