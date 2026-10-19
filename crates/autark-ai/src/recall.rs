//! Recall: what the user said to Kith before (the week of conversation Kith keeps) and what is in their notes (a
//! folder of Markdown, `~/Notes` unless Notes was pointed elsewhere), found by the words of the question. It reads
//! only; the result goes to the model on this laptop and nowhere else, and the caller closes the web for the rest of
//! the question, as for a file read.
//!
//! Notes are the user's own, and people keep things in them they shouldn't: a note named for a token or a password
//! isn't read at all, and any long run of key-like characters is replaced, so a key pasted into an ordinary note
//! doesn't reach the model or the answer.

use std::path::{Path, PathBuf};

use crate::chat::Chat;

/// Words that say nothing about what to find.
const COMMON: [&str; 40] = [
    "the", "and", "what", "when", "where", "which", "who", "how", "did", "does", "was", "were", "are", "you", "your", "for", "that", "this", "with", "about",
    "from", "have", "has", "had", "said", "say", "tell", "told", "remember", "notes", "note", "last", "week", "earlier", "decided", "discussed", "talked", "any", "can", "why",
];
/// A note whose name has one of these in it is never read.
const SECRET_NAMES: [&str; 9] = ["token", "password", "passwd", "secret", "recovery", "credential", "passphrase", "private", "apikey"];
/// Notes, folders and bytes looked at, so a huge vault can't stall an answer.
const MAX_NOTES: usize = 2000;
const MAX_DEPTH: usize = 6;
const MAX_BYTES: u64 = 200 * 1024;
/// How many of each kind a recall returns, and how wide a snippet is, in characters.
const TOP: usize = 3;
const WIDTH: usize = 300;

struct Hit {
    score: usize,
    /// Seconds since the epoch, for ties: the newer first.
    time: u64,
    line: String,
}

/// The words of the question worth searching for.
pub fn words(question: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for word in question.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if word.chars().count() >= 3 && !COMMON.contains(&word) && !found.iter().any(|seen| seen == word) {
            found.push(word.to_owned());
        }
    }
    found
}

/// How many of the words the text has.
fn score(text: &str, words: &[String]) -> usize {
    let text = text.to_lowercase();
    words.iter().filter(|word| text.contains(word.as_str())).count()
}

/// What a hit has to score: half the words, at least one.
fn needed(words: &[String]) -> usize {
    words.len().div_ceil(2).max(1)
}

/// Long runs of key-like characters, which are probably secrets, replaced.
fn mask(text: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= 24 && run.chars().any(|c| c.is_ascii_digit()) {
            out.push('…');
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '=' | '+' | '/') {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// About `WIDTH` characters around the first of the words in the text, on one line.
fn snippet(text: &str, words: &[String]) -> String {
    let flat = mask(&text.split_whitespace().collect::<Vec<_>>().join(" "));
    let lower = flat.to_lowercase();
    let at = words.iter().filter_map(|word| lower.find(word.as_str())).min().unwrap_or(0);
    // Back up to the start of a character, then to a word.
    let mut start = at.saturating_sub(WIDTH / 3);
    while !flat.is_char_boundary(start) {
        start -= 1;
    }
    if start > 0 {
        start = flat[start..].find(' ').map_or(start, |space| start + space + 1);
    }
    let body: String = flat[start..].chars().take(WIDTH).collect();
    let (lead, tail) = (if start > 0 { "… " } else { "" }, if flat[start..].chars().count() > WIDTH { " …" } else { "" });
    format!("{lead}{}{tail}", body.trim())
}

fn secret_name(name: &str) -> bool {
    let name = name.to_lowercase();
    SECRET_NAMES.iter().any(|secret| name.contains(secret))
}

/// The vault: Notes' own setting, else the folder it last opened, else `~/Notes`.
fn vault() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/state"));
    let chosen = std::fs::read_to_string(config.join("autark/notes.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|value| value.get("vault").and_then(|vault| vault.as_str()).map(str::to_owned))
        .map(|vault| vault.strip_prefix("~/").map_or_else(|| PathBuf::from(&vault), |rest| home.join(rest)))
        .or_else(|| std::fs::read_to_string(state.join("autark/notes-vault")).ok().map(|text| PathBuf::from(text.trim())));
    chosen.filter(|path| path.is_dir()).unwrap_or_else(|| home.join("Notes"))
}

/// Notes in `root` that have enough of the words, best first. Dot-folders, secret-named notes and big files are skipped.
fn notes(root: &Path, words: &[String]) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut seen = 0;
    let mut stack = vec![(root.to_owned(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if name.starts_with('.') || entry.file_type().map(|kind| kind.is_symlink()).unwrap_or(true) {
                continue;
            }
            if meta.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let Some(title) = name.strip_suffix(".md") else { continue };
            if secret_name(title) || meta.len() > MAX_BYTES || seen >= MAX_NOTES {
                continue;
            }
            seen += 1;
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            // A match in the title counts double: a note is usually about what it's called.
            let found = score(&text, words) + score(title, words);
            if found >= needed(words) {
                let time = meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |since| since.as_secs());
                hits.push(Hit { score: found, time, line: format!("- {title}: {}", snippet(&text, words)) });
            }
        }
    }
    best(hits)
}

/// "today", "yesterday" or "3 days ago".
fn ago(time: u64, now: u64) -> String {
    match now.saturating_sub(time) / 86_400 {
        0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        days => format!("{days} days ago"),
    }
}

/// What the user and Kith said that has enough of the words, best first.
fn chats(chat: &Chat, words: &[String], now: u64) -> Vec<Hit> {
    let hits = chat
        .spoken()
        .into_iter()
        .filter(|(_, _, text)| !text.is_empty())
        .filter_map(|(time, who, text)| {
            let found = score(&text, words);
            (found >= needed(words)).then(|| Hit { score: found, time, line: format!("- {}, {who}: {}", ago(time, now), snippet(&text, words)) })
        })
        .collect();
    best(hits)
}

fn best(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(b.time.cmp(&a.time)));
    hits.truncate(TOP);
    hits
}

fn report(notes: &[Hit], chats: &[Hit]) -> String {
    let mut out = Vec::new();
    if !notes.is_empty() {
        out.push(format!("From the user's notes:\n{}", notes.iter().map(|hit| hit.line.as_str()).collect::<Vec<_>>().join("\n")));
    }
    if !chats.is_empty() {
        out.push(format!("From earlier conversations with Kith:\n{}", chats.iter().map(|hit| hit.line.as_str()).collect::<Vec<_>>().join("\n")));
    }
    if out.is_empty() { "Nothing in the user's notes or earlier conversations matches.".to_owned() } else { out.join("\n\n") }
}

/// Recall for a question or search words: matches from the notes and earlier chats, as text for the model.
pub fn search(question: &str) -> String {
    let words = words(question);
    if words.is_empty() {
        return "Nothing to search for: the question has no distinctive words.".to_owned();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    report(&notes(&vault(), &words), &chats(&Chat::load(), &words, now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kith-recall-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_words_are_the_distinctive_ones() {
        assert_eq!(words("What did we decide about the Wayfire config last week?"), ["decide", "wayfire", "config"]);
        assert!(words("what did we say").is_empty());
    }

    #[test]
    fn notes_match_on_their_words_and_skip_what_is_private() {
        let dir = folder("notes");
        std::fs::write(dir.join("Kernel plan.md"), "Update to 7.2.11 after the CVE report. Keep 7.1.8 as the fallback.").unwrap();
        std::fs::write(dir.join("Oauth Token.md"), "kernel token abc").unwrap();
        std::fs::write(dir.join("jev.md"), "kernel notes: key apikey_2255e4849a7af4a4e08a9f8abbbeac89f8b_8657d2fce2 and more").unwrap();
        std::fs::create_dir_all(dir.join(".trash")).unwrap();
        std::fs::write(dir.join(".trash/old.md"), "kernel in the trash").unwrap();
        std::fs::write(dir.join("elsewhere.txt"), "kernel in a text file").unwrap();
        let hits = notes(&dir, &words("what's the kernel plan"));
        let lines: Vec<&str> = hits.iter().map(|hit| hit.line.as_str()).collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("- Kernel plan: ") && lines[0].contains("7.2.11"), "the note named for the words comes first");
        assert!(lines[1].starts_with("- jev: ") && lines[1].contains("kernel notes: key …") && !lines[1].contains("apikey_"), "{lines:?}");
        assert!(!lines.iter().any(|line| line.contains("trash") || line.contains("token") || line.contains("text file")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_snippet_is_one_line_around_the_match() {
        let text = format!("{}needle sits here{}", "before ".repeat(60), " after".repeat(60));
        let found = snippet(&text, &["needle".to_owned()]);
        assert!(found.starts_with("… ") && found.ends_with(" …") && found.contains("needle sits here") && !found.contains('\n'));
        assert!(found.chars().count() <= WIDTH + 6);
        assert_eq!(snippet("short note", &["note".to_owned()]), "short note");
    }

    #[test]
    fn keys_are_masked_but_ordinary_words_are_not() {
        assert_eq!(mask("key sk_live_51Habcdefghijklmnopqrstuvwx9 ok"), "key … ok");
        assert_eq!(mask("a-very-long-hyphenated-description-without-digits stays"), "a-very-long-hyphenated-description-without-digits stays");
    }

    #[test]
    fn nothing_found_says_so_and_dates_are_plain() {
        assert_eq!(report(&[], &[]), "Nothing in the user's notes or earlier conversations matches.");
        assert_eq!((ago(100_000, 100_000), ago(100_000 - 90_000, 100_000), ago(0, 3 * 86_400 + 5)), ("today".to_owned(), "yesterday".to_owned(), "3 days ago".to_owned()));
        assert_eq!(needed(&words("kernel update plan")), 2);
        assert_eq!(needed(&[]), 1);
    }
}
