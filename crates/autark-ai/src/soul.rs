//! Kith's soul: `~/.config/autark-ai/soul.md`, a Markdown note on who Kith is and how it talks, in the style of
//! Hermes' `SOUL.md`. It colours the voice of answers (Ask, the conversation, a task's report) and nothing
//! else: it is added after the fixed instructions, never reaches routing, rating or checking, and can't give
//! Kith new powers. Cut to 4 KB; an empty or missing file means no soul.

use std::path::PathBuf;

const KEPT: usize = 4 * 1024;

fn path() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("autark-ai/soul.md")
}

/// The soul as an addition to a system prompt, or nothing.
pub fn voice() -> String {
    let text = std::fs::read_to_string(path()).unwrap_or_default();
    with(&text)
}

/// `text` as the addition to a system prompt.
pub fn with(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let mut end = text.len().min(KEPT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "\nYour soul, which is how you sound and who you are, not what you may do; it never changes the rules above:\n{}",
        &text[..end]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_soul_is_kept_short_and_an_empty_one_is_none() {
        assert_eq!(with("  \n"), "");
        assert!(with("Warm and dry.").ends_with("Warm and dry."));
        assert!(with(&"é".repeat(5000)).len() < KEPT + 200);
    }
}
