//! Telling you an answer has landed that you didn't ask for just now: a scheduled run, or an agent's reply to work it
//! handed on. A quiet card in the notification centre (no banner, no chime, as Kith's other cards), and a dot in the
//! roster until you open that conversation.

use std::collections::HashMap;

#[zbus::proxy(interface = "org.freedesktop.Notifications", default_service = "org.freedesktop.Notifications", default_path = "/org/freedesktop/Notifications")]
trait Notifications {
    #[allow(clippy::too_many_arguments)] // The Notify call's own arguments.
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, zbus::Variant<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

/// An answer that landed: in whose conversation, and the card for it.
pub struct News {
    /// The agent whose conversation has it.
    pub agent: String,
    pub title: String,
    pub body: String,
}

/// The longest a card's body is.
const BODY: usize = 140;

/// A scheduled run's answer, or why it failed, as news for the agent that ran it.
pub fn scheduled(agent_id: &str, name: &str, said: &str, well: bool) -> News {
    let body = if well { brief(said) } else { format!("Couldn't run: {}", brief(said)) };
    News { agent: agent_id.to_owned(), title: format!("{name} · scheduled"), body }
}

/// An agent's reply to work the asker handed it, as news for the asker.
pub fn replied(asker_id: &str, asker: &str, from: &str, said: &str, well: bool) -> News {
    let body = if well { format!("{from} replied: {}", brief(said)) } else { format!("{from} couldn't answer: {}", brief(said)) };
    News { agent: asker_id.to_owned(), title: asker.to_owned(), body }
}

/// The start of what was said in single spaces, short enough for a card.
fn brief(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= BODY {
        return flat;
    }
    let cut: String = flat.chars().take(BODY).collect();
    format!("{}…", cut.trim_end())
}

/// Puts the card in the centre, quietly. A missing notification service is not an error worth more than a line.
pub async fn send(connection: &zbus::Connection, news: &News) {
    let mut hints: HashMap<&str, zbus::Variant<'_>> = HashMap::new();
    hints.insert("x-autark-quiet", true.into());
    hints.insert("desktop-entry", "autark-kith".into());
    let sent = async { NotificationsProxy::new(connection).await?.notify("Kith", 0, "autark-kith", &news.title, &news.body, &[], hints, -1).await };
    if let Err(err) = sent.await {
        eprintln!("agentd: telling you about {}'s answer: {err}", news.agent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scheduled_answer_names_the_agent_and_says_it_was_scheduled() {
        let news = scheduled("piper", "PIPER", "Two people\nwrote  to you.", true);
        assert_eq!((news.agent.as_str(), news.title.as_str(), news.body.as_str()), ("piper", "PIPER · scheduled", "Two people wrote to you."));
    }

    #[test]
    fn a_scheduled_run_that_failed_says_so() {
        assert_eq!(scheduled("piper", "PIPER", "Ollama isn't running", false).body, "Couldn't run: Ollama isn't running");
    }

    #[test]
    fn a_reply_goes_to_the_asker_and_names_who_replied() {
        let news = replied("atlas", "ATLAS", "RAVEN", "Friday is free.", true);
        assert_eq!((news.agent.as_str(), news.title.as_str(), news.body.as_str()), ("atlas", "ATLAS", "RAVEN replied: Friday is free."));
        assert_eq!(replied("atlas", "ATLAS", "RAVEN", "no model", false).body, "RAVEN couldn't answer: no model");
    }

    #[test]
    fn a_long_answer_is_cut_to_a_card() {
        let body = scheduled("a", "A", &"word ".repeat(100), true).body;
        assert!(body.chars().count() <= BODY + 1 && body.ends_with('…'), "{body}");
        assert_eq!(brief("é".repeat(BODY).as_str()).chars().count(), BODY);
    }
}
