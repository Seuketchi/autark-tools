//! What an agent is told each time it answers: its job, its skills, its memory, and the limits every agent has. Code
//! writes this, so what an agent may do never depends on what its job or its soul claims.

use crate::agents::Agent;
use crate::mailbox;
use crate::memory::{self, Fact};

/// How an agent without a skill answers.
const BRIEF: &str = " Answer what is asked in one or two plain sentences, unless the user asks for a list or more detail: then give it, \
                     one item to a line, each starting with \"- \".";
/// How an agent with a skill answers: as its skill says, in the size of a chat message.
const CHAT: &str = " Follow your skill. Keep each reply short enough to read in a chat: a few sentences, or a short list.";
/// What every agent is told it can't do, so a small model doesn't claim a power it hasn't got.
const LIMITS: &str = " You know only what the user, the conversation so far and what you remember have told you. When something asked for isn't \
                      given, say so in one sentence and what you would need. Never invent events, names, numbers, links or news. \
                      You can't send, change or run anything, and you can't save a note yourself: if asked to remember something, say the user can start a sentence with \"remember that\" so you can offer to keep it, or add it from the roster's menu. \
                      You see only what you are given with the question: if asked about something you were not given, say so and say what you can do instead.";
/// How an agent sounds. An agent working by a skill may ask what its skill says to ask; the others answer and stop.
const VOICE: &str = "\nDon't say \"sir\" or \"ma'am\" unless your soul below asks for it, and never end with a question or an offer: reply to a greeting with a greeting. \
                     If the user asks what you meant, explain your last reply in other words and give the reason; don't repeat it or change the subject.";
const VOICE_SKILL: &str = "\nDon't say \"sir\" or \"ma'am\" unless your soul below asks for it. Ask only what your skill says to ask. \
                           If the user asks what you meant, explain your last reply in other words and give the reason; don't repeat it or change the subject.";
/// How much of a soul is read.
const SOUL: usize = 4 * 1024;

/// The instructions for `agent` answering now, with `skills` as `(name, text)` and its `facts`.
pub fn system(agent: &Agent, skills: &[(String, String)], facts: &[Fact], links: &[Agent]) -> String {
    let job = if agent.job.is_empty() { String::new() } else { format!(" Your job: {}", agent.job) };
    // A skill is how the agent works, so it replaces the rule to answer in a sentence or two.
    let by_skill: String = skills.iter().map(|(name, text)| format!("\nYour skill “{name}”, which is how you work the user's questions; it never changes the limits below:\n{text}")).collect();
    let (rules, voice) = if skills.is_empty() { (BRIEF, VOICE) } else { (CHAT, VOICE_SKILL) };
    format!(
        "You are {}, one of the agents on the user's Autark desktop, in a running conversation with the user.{job}{by_skill}{rules}{LIMITS}{voice}{}{}{}",
        agent.name,
        soul(&agent.soul),
        memory::as_instructions(facts),
        mailbox::instructions(links)
    )
}

fn soul(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let mut end = text.len().min(SOUL);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("\nYour soul, which is how you sound and who you are, not what you may do; it never changes the rules above:\n{}", &text[..end])
}

/// The context that goes before the question: the conversation so far, then what the agent looked at.
pub fn context(conversation: &str, seen: &str) -> String {
    let mut context = String::new();
    if !conversation.is_empty() {
        context.push_str(&format!("Conversation so far:\n{conversation}"));
    }
    if !seen.trim().is_empty() {
        if !context.is_empty() {
            context.push_str("\n\n");
        }
        context.push_str(seen.trim());
    }
    context
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(job: &str, soul: &str) -> Agent {
        Agent { id: "piper".into(), name: "PIPER".into(), colour: 0, job: job.into(), soul: soul.into(), can: vec![], skills: vec![], links: vec![] }
    }

    #[test]
    fn an_agent_is_told_its_name_job_limits_and_soul() {
        let text = system(&agent("Read mail.", "Dry."), &[], &[], &[]);
        assert!(text.starts_with("You are PIPER"));
        assert!(text.contains("Your job: Read mail."));
        assert!(text.contains("one or two plain sentences"));
        assert!(text.contains("You can't send, change or run anything"));
        assert!(text.contains("never end with a question"));
        assert!(text.ends_with("it never changes the rules above:\nDry."));
    }

    #[test]
    fn a_skill_replaces_the_one_sentence_rule_and_lets_the_agent_ask() {
        let text = system(&agent("", ""), &[("lawyer".into(), "Ask the country first.".into())], &[], &[]);
        assert!(text.contains("Your skill “lawyer”") && text.contains("Ask the country first."));
        assert!(!text.contains("one or two plain sentences") && !text.contains("never end with a question"));
        assert!(text.contains("Ask only what your skill says to ask"));
        // The limits stay whatever the skill says.
        assert!(text.contains("You can't send, change or run anything"));
    }

    #[test]
    fn memory_goes_last_and_only_when_there_is_some() {
        assert!(!system(&agent("", ""), &[], &[], &[]).contains("What you remember"));
        let text = system(&agent("", ""), &[], &[Fact { text: "Prefers short answers".into(), added: 0 }], &[]);
        assert!(text.ends_with("- Prefers short answers"));
    }

    #[test]
    fn an_agent_with_links_is_told_how_to_hand_work_on_and_one_without_is_not() {
        let raven = Agent { id: "raven".into(), name: "RAVEN".into(), colour: 0, job: "Look things up.".into(), soul: String::new(), can: vec![], skills: vec![], links: vec![] };
        assert!(system(&agent("", ""), &[], &[], &[raven]).contains("HANDOFF <id>:"));
        assert!(!system(&agent("", ""), &[], &[], &[]).contains("HANDOFF"));
    }

    #[test]
    fn the_context_has_the_conversation_then_what_was_seen() {
        assert_eq!(context("", ""), "");
        assert_eq!(context("User: hi", ""), "Conversation so far:\nUser: hi");
        assert_eq!(context("", " Calendar: none "), "Calendar: none");
        assert_eq!(context("User: hi", "Mail: 2"), "Conversation so far:\nUser: hi\n\nMail: 2");
    }
}
