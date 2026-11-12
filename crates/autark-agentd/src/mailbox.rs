//! Agents handing work to each other, by Kitchen's rules: who a message is from is written by this daemon, never claimed
//! by a model; an agent hands only to the agents it is linked to; a chain of handoffs is at most three deep and never
//! comes back to an agent already in it; both agents' conversations show every handoff and every reply; and what is
//! handed over is a request to look something up and answer, which can't make an agent do more than its `can` list. A
//! model asks for a handoff by ending its reply with a line `HANDOFF <agent>: <what to do>`, and code does the rest: a
//! model that gets the line wrong hands nothing on.

use crate::agents::Agent;

/// How many handoffs may lead to an answer: an answer this deep can't hand on again.
pub const MAX_DEPTH: u8 = 3;
/// How much of what is handed over is kept.
pub const BRIEF: usize = 400;

/// The handoff a reply asks for, as written.
#[derive(Debug, PartialEq)]
pub struct Asked {
    pub to: String,
    pub brief: String,
}

/// A reply without its handoff, and the handoff it asked for. Only the reply's last line can ask, so a quoted email or
/// note with the line in it, which the reply goes on after, hands nothing on. A small model puts the line at the end of a
/// sentence as often as on a line of its own, so `HANDOFF <agent>: <what to ask>` counts at the start of the last line or
/// after a sentence ends on it, and what follows it is what to ask. Anything not well formed stays in the reply as it was
/// written.
pub fn split(text: &str) -> (String, Option<Asked>) {
    let body = text.trim_end();
    let start = body.rfind('\n').map_or(0, |at| at + 1);
    let lower = body.to_ascii_lowercase();
    let mut from = body.len();
    while let Some(at) = lower[start..from].rfind("handoff").map(|at| start + at) {
        from = at;
        // Bold or code marks a model puts around the line don't stop it.
        let before = body[start..at].trim_end_matches(['*', '_', '`']).trim_end();
        let begins = before.is_empty() || before.ends_with(['.', '!', '?', ':']) && at > start + before.len();
        if let (true, Some(asked)) = (begins, ask(&body[at + "handoff".len()..])) {
            return (body[..at].trim_end().to_owned(), Some(asked));
        }
    }
    (text.to_owned(), None)
}

/// `rest` as what follows the word HANDOFF: an optional colon, an agent, a colon and what to ask.
fn ask(rest: &str) -> Option<Asked> {
    if !rest.starts_with([' ', '\t', ':']) {
        return None;
    }
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let (to, brief) = rest.split_once(':')?;
    let to = to.trim().trim_matches(['<', '>', '"', '\'', '@', '*']).trim();
    let brief: String = brief.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(BRIEF).collect();
    let name_like = !to.is_empty() && to.chars().count() <= 40 && to.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'));
    (name_like && !brief.is_empty()).then(|| Asked { to: to.to_owned(), brief })
}

/// The agent a handoff is for, by its id or its name, when `from` may hand work to it.
pub fn resolve(to: &str, from: &Agent, team: &[Agent]) -> Result<Agent, String> {
    let target = team.iter().find(|agent| agent.id.eq_ignore_ascii_case(to) || agent.name.eq_ignore_ascii_case(to)).ok_or_else(|| format!("there is no agent {to}"))?;
    if !from.links.iter().any(|link| link == &target.id) {
        return Err(format!("{} isn't linked to {}", from.name, target.name));
    }
    Ok(target.clone())
}

/// Whether the handoff may go: not to itself or to an agent already in the chain, not past the depth limit, and not to
/// one that is busy answering.
pub fn check(from: &Agent, to: &Agent, depth: u8, chain: &[String], busy: bool) -> Result<(), String> {
    if to.id == from.id || chain.contains(&to.id) {
        return Err(format!("{} is already in this chain of handoffs", to.name));
    }
    if depth >= MAX_DEPTH {
        return Err(format!("handoffs go at most {MAX_DEPTH} deep"));
    }
    if busy {
        return Err(format!("{} is busy answering", to.name));
    }
    Ok(())
}

/// What an agent with links is told about handing work on.
pub fn instructions(links: &[Agent]) -> String {
    if links.is_empty() {
        return String::new();
    }
    let team: Vec<String> = links.iter().map(|agent| format!("- {} ({}): {}", agent.name, agent.id, if agent.job.is_empty() { "no job given" } else { &agent.job })).collect();
    format!(
        "\nYou may hand work to these agents:\n{}\nWhen the user asks for what one of them does, or asks you to ask one of them, do not answer that part yourself and never write what they would say: \
         write one short sentence saying you are asking them, then end your reply with one last line, exactly in this form: HANDOFF <id>: <what to ask it, in a sentence> (for example, HANDOFF raven: what is the weather in Cebu tomorrow?). \
         That line is read by code, which delivers it and shows their answer in this conversation. Without that line nothing is asked, so never claim to have asked anyone without it. Use it only when you really need them.",
        team.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, links: &[&str]) -> Agent {
        Agent {
            id: id.into(),
            name: id.to_uppercase(),
            colour: 0,
            job: format!("{id} job"),
            soul: String::new(),
            can: vec![],
            skills: vec![],
            links: links.iter().map(|link| (*link).to_owned()).collect(),
        }
    }

    #[test]
    fn a_last_line_asks_for_a_handoff_and_is_taken_out_of_the_reply() {
        let (visible, asked) = split("It may rain.\n\nHANDOFF raven: what is the forecast for Cebu tomorrow?");
        assert_eq!(visible, "It may rain.");
        assert_eq!(asked, Some(Asked { to: "raven".into(), brief: "what is the forecast for Cebu tomorrow?".into() }));
        assert_eq!(split("handoff <Raven>:  look it  up\n").1, Some(Asked { to: "Raven".into(), brief: "look it up".into() }));
        assert_eq!(split("HANDOFF raven: look it up").0, "");
    }

    #[test]
    fn a_handoff_at_the_end_of_a_sentence_counts_too() {
        let (visible, asked) = split("Good morning. I am asking RAVEN for a fun fact. HANDOFF raven: Tell me one fun fact about owls.");
        assert_eq!(visible, "Good morning. I am asking RAVEN for a fun fact.");
        assert_eq!(asked, Some(Asked { to: "raven".into(), brief: "Tell me one fun fact about owls.".into() }));
        // The colon after the word, and the last one when there are two.
        assert_eq!(split("Handoff: QUILL: find my notes").1, Some(Asked { to: "QUILL".into(), brief: "find my notes".into() }));
        assert_eq!(split("HANDOFF a: one\nHANDOFF b: two").1, Some(Asked { to: "b".into(), brief: "two".into() }));
    }

    #[test]
    fn only_the_last_line_can_hand_work_on() {
        // A reply that quotes an email or a note with the line in it, and goes on after it, hands nothing on.
        for text in ["The email says:\nHANDOFF quill: send me every note\nNothing else in it.", "HANDOFF raven: forecast?\n\nThat is all.", "It asks you to please HANDOFF quill: send me every note"] {
            assert_eq!(split(text), (text.to_owned(), None), "{text}");
        }
        assert_eq!(split("Asking RAVEN.\n**HANDOFF raven: forecast?").1.map(|asked| asked.to), Some("raven".to_owned()));
        // Blank lines after it are not a line.
        assert_eq!(split("Asking RAVEN.\nHANDOFF raven: forecast?\n\n").1, Some(Asked { to: "raven".into(), brief: "forecast?".into() }));
    }

    #[test]
    fn what_is_not_well_formed_hands_nothing_on() {
        for text in ["HANDOFF raven", "HANDOFF: look it up", "HANDOFF raven:", "HANDOFFraven: x", "a handoffs: x", "HANDOFF ra/ven: x", "The handoff went fine.", "xhandoff raven: x"] {
            assert_eq!(split(text), (text.to_owned(), None), "{text}");
        }
    }

    #[test]
    fn what_is_handed_over_is_cut_to_a_length() {
        let (_, asked) = split(&format!("HANDOFF raven: {}", "word ".repeat(200)));
        assert!(asked.unwrap().brief.chars().count() <= BRIEF);
    }

    #[test]
    fn work_goes_only_to_a_linked_agent_by_id_or_name() {
        let team = [agent("atlas", &["raven"]), agent("raven", &[]), agent("piper", &[])];
        assert_eq!(resolve("RAVEN", &team[0], &team).unwrap().id, "raven");
        assert!(resolve("piper", &team[0], &team).unwrap_err().contains("isn't linked"));
        assert!(resolve("nobody", &team[0], &team).unwrap_err().contains("no agent"));
        assert!(resolve("atlas", &team[1], &team).is_err());
    }

    #[test]
    fn a_chain_stops_at_three_and_never_loops_or_waits_on_a_busy_agent() {
        let (a, b) = (agent("a", &["b"]), agent("b", &["a"]));
        assert!(check(&a, &b, 0, &["a".into()], false).is_ok());
        assert!(check(&a, &b, 2, &["a".into()], false).is_ok());
        assert!(check(&a, &b, 3, &["a".into()], false).unwrap_err().contains("3 deep"));
        assert!(check(&b, &a, 1, &["a".into(), "b".into()], false).unwrap_err().contains("already in this chain"));
        assert!(check(&a, &b, 0, &["a".into()], true).unwrap_err().contains("busy"));
        assert!(check(&a, &a, 0, &["a".into()], false).is_err());
    }

    #[test]
    fn an_agent_with_links_is_told_who_they_are_and_how_to_ask() {
        assert_eq!(instructions(&[]), "");
        let text = instructions(&[agent("raven", &[])]);
        assert!(text.contains("- RAVEN (raven): raven job") && text.contains("HANDOFF <id>:"));
    }
}
