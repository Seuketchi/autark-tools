//! Telling an agent to remember something. Code reads the sentence, not the model: a small model would say it had saved
//! a fact it hadn't. A sentence that starts by asking to remember becomes a fact for you to save or not.

/// How such a sentence may begin, longest first.
const ASKS: [&str; 5] = ["can you please ", "could you please ", "can you ", "could you ", "please "];
const LEADS: [&str; 11] =
    ["remember that", "remember this:", "remember:", "remember", "don't forget that", "dont forget that", "don't forget", "dont forget", "keep in mind that", "keep in mind", "note that"];
/// What follows "remember" in a question or a memory of the past, which isn't a fact to keep.
const NOT_FACTS: [&str; 7] = ["when", "what", "how", "why", "who", "where", "if"];
/// A fact is at most this long; see `memory`.
const SHORTEST: usize = 3;

/// The fact in `said`, if it asks to remember one.
pub fn fact(said: &str) -> Option<String> {
    let said = said.trim();
    let lower = said.to_lowercase();
    let mut at = 0;
    for ask in ASKS {
        if lower.starts_with(ask) {
            at = ask.len();
            break;
        }
    }
    let lead = LEADS.iter().find(|lead| lower[at..].starts_with(*lead))?;
    // `lower` and `said` are the same length up to the lead, which is ASCII, so this is a character boundary.
    let rest = said.get(at + lead.len()..)?;
    if !rest.is_empty() && !rest.starts_with([' ', ':', ',']) {
        return None;
    }
    let rest = rest.trim().trim_start_matches([':', ',']).trim();
    let first = rest.split_whitespace().next()?.to_lowercase();
    if rest.chars().count() < SHORTEST || rest.ends_with('?') || NOT_FACTS.contains(&first.as_str()) {
        return None;
    }
    Some(rest.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_to_remember_gives_the_fact() {
        assert_eq!(fact("remember that my wife's birthday is May 3").as_deref(), Some("my wife's birthday is May 3"));
        assert_eq!(fact("Please remember: I like oat milk").as_deref(), Some("I like oat milk"));
        assert_eq!(fact("can you please remember I'm vegetarian").as_deref(), Some("I'm vegetarian"));
        assert_eq!(fact("Don't forget that the gate code is 4821").as_deref(), Some("the gate code is 4821"));
        assert_eq!(fact("keep in mind I work nights").as_deref(), Some("I work nights"));
    }

    #[test]
    fn questions_and_other_sentences_are_not_facts() {
        assert_eq!(fact("do you remember my name?"), None);
        assert_eq!(fact("remember when we talked about taxes"), None);
        assert_eq!(fact("remember what I said?"), None);
        assert_eq!(fact("remember"), None);
        assert_eq!(fact("remember it"), None);
        assert_eq!(fact("remembering things is hard"), None);
        assert_eq!(fact("who are you"), None);
    }

    #[test]
    fn text_with_other_alphabets_does_not_break_it() {
        assert_eq!(fact("İ remember that x"), None);
        assert_eq!(fact("remember that café opens at 7").as_deref(), Some("café opens at 7"));
    }
}
