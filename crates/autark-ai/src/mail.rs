//! Kith's mail: the newest messages in your inbox, who they are from and what they are about, read through the
//! Mail service (`org.autark.Mail1`, which the session bus starts when asked). Kith holds no mail password: the
//! account was signed in once, by `autark-accounts`, and the service signs in for each read.
//!
//! Read-only, and only the list: a message's body is not fetched, so nothing is marked read and nothing long is
//! handed to the model. The list stays on this laptop. A question that is only about mail is answered from the list by
//! code, not by the model; the loop closes the web after a mail read, as after any private read.

use serde_json::Value;

/// How many of the newest messages are looked at, and the most one answer lists.
const LOOKED_AT: u32 = 30;
const LISTED: usize = 10;

#[zbus::proxy(interface = "org.autark.Mail1", default_service = "org.autark.Mail1", default_path = "/org/autark/Mail1")]
trait Mail {
    fn accounts(&self) -> zbus::Result<String>;
    fn recent(&self, account: &str, folder: &str, count: u32) -> zbus::Result<String>;
}

/// One message in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Letter {
    pub from: String,
    pub subject: String,
    pub date: String,
    pub unread: bool,
}

/// Whether the question wants only what is unread.
pub fn only_unread(question: &str) -> bool {
    let question = question.to_lowercase();
    ["unread", "new email", "new mail", "new message", "didn't read", "not read"].iter().any(|word| question.contains(word))
}

/// The name of whoever sent it: `"Ada Lovelace" <ada@x.org>` is Ada Lovelace, a bare address stays as it is.
fn sender(from: &str) -> String {
    let from = from.trim();
    let name = from.split('<').next().unwrap_or("").trim().trim_matches('"').trim();
    let address = from.split_once('<').map_or(from, |(_, rest)| rest.split('>').next().unwrap_or(rest)).trim();
    clean(if name.is_empty() { address } else { name }, 40)
}

/// Text from a message is only ever shown as one plain line: no control characters, cut at a width.
fn clean(text: &str, width: usize) -> String {
    let line: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= width {
        return line;
    }
    format!("{}…", line.chars().take(width).collect::<String>().trim_end())
}

/// The messages in the service's `Recent` reply, newest first.
pub fn letters(reply: &str) -> Result<Vec<Letter>, String> {
    let list: Vec<Value> = serde_json::from_str(reply).map_err(|_| "the Mail service answered in a way Kith doesn't understand".to_owned())?;
    Ok(list
        .iter()
        .map(|item| {
            let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("");
            let subject = clean(text("subject"), 90);
            Letter { from: sender(text("from")), subject: if subject.is_empty() { "(no subject)".to_owned() } else { subject }, date: clean(text("date"), 20), unread: item.get("unread").and_then(Value::as_bool).unwrap_or(false) }
        })
        .collect())
}

/// The list as lines: the newest first, unread ones marked, only the unread ones if that is what was asked.
pub fn describe(account: &str, letters: &[Letter], unread_only: bool) -> String {
    let unread = letters.iter().filter(|letter| letter.unread).count();
    let shown: Vec<&Letter> = letters.iter().filter(|letter| letter.unread || !unread_only).take(LISTED).collect();
    let head = if unread_only {
        format!("Unread in {account}, among the newest {} messages:", letters.len())
    } else {
        format!("Newest in {account} ({unread} unread among the newest {}):", letters.len())
    };
    if shown.is_empty() {
        return if letters.is_empty() { format!("{account} has no messages.") } else { format!("Nothing unread in {account}, among the newest {} messages.", letters.len()) };
    }
    let lines: Vec<String> = shown.iter().map(|letter| format!("• {}{} — {}: {}", if letter.unread { "(unread) " } else { "" }, letter.date, letter.from, letter.subject)).collect();
    format!("{head}\n{}", lines.join("\n"))
}

/// The inbox of every mail account for a question about it. Errors say what to do and never carry a password.
pub async fn answer(connection: &zbus::Connection, question: &str) -> Result<String, String> {
    let unread_only = only_unread(question);
    let mail = MailProxy::new(connection).await.map_err(|err| format!("the Mail service: {err}"))?;
    let accounts: Vec<Value> = serde_json::from_str(&mail.accounts().await.map_err(|err| format!("the Mail service: {err}"))?).map_err(|_| "the Mail service answered in a way Kith doesn't understand".to_owned())?;
    if accounts.is_empty() {
        return Err("no mail account is connected (add one with autark-accounts)".to_owned());
    }
    let mut parts = Vec::new();
    for account in &accounts {
        let id = account.get("id").and_then(Value::as_str).unwrap_or("");
        let name = account.get("email").and_then(Value::as_str).filter(|email| !email.is_empty()).or_else(|| account.get("name").and_then(Value::as_str)).unwrap_or(id);
        let reply = mail.recent(id, "INBOX", LOOKED_AT).await.map_err(|err| format!("{name}: {err}"))?;
        parts.push(describe(name, &letters(&reply)?, unread_only));
    }
    Ok(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"[
        {"uid":9,"from":"\"Ada Lovelace\" <ada@x.org>","subject":"Lunch\nthis week?","date":"10:42","unread":true},
        {"uid":8,"from":"billing@shop.example","subject":"","date":"Oct 18","unread":false},
        {"uid":7,"from":"News <news@y.org>","subject":"Weekly digest","date":"Oct 17","unread":true}
    ]"#;

    #[test]
    fn the_list_names_who_and_what_on_one_line_each() {
        let list = letters(REPLY).unwrap();
        assert_eq!(list[0], Letter { from: "Ada Lovelace".into(), subject: "Lunch this week?".into(), date: "10:42".into(), unread: true });
        assert_eq!(list[1].from, "billing@shop.example");
        assert_eq!(list[1].subject, "(no subject)");
        let text = describe("me@gmail.com", &list, false);
        assert_eq!(
            text,
            "Newest in me@gmail.com (2 unread among the newest 3):\n• (unread) 10:42 — Ada Lovelace: Lunch this week?\n• Oct 18 — billing@shop.example: (no subject)\n• (unread) Oct 17 — News: Weekly digest"
        );
    }

    #[test]
    fn unread_only_leaves_out_what_was_read() {
        let list = letters(REPLY).unwrap();
        let text = describe("me@gmail.com", &list, true);
        assert!(text.starts_with("Unread in me@gmail.com"), "{text}");
        assert!(text.contains("Ada Lovelace") && text.contains("News") && !text.contains("billing"), "{text}");
        let read: Vec<Letter> = list.iter().cloned().map(|l| Letter { unread: false, ..l }).collect();
        assert_eq!(describe("me@gmail.com", &read, true), "Nothing unread in me@gmail.com, among the newest 3 messages.");
        assert_eq!(describe("me@gmail.com", &[], false), "me@gmail.com has no messages.");
    }

    #[test]
    fn a_question_asks_for_unread_in_plain_words() {
        assert!(only_unread("any unread emails?"));
        assert!(only_unread("do I have new mail"));
        assert!(!only_unread("what's in my inbox"));
    }

    #[test]
    fn what_a_message_says_is_one_plain_line_cut_to_fit() {
        assert_eq!(clean("a\u{1b}[31m\n\tb   c", 40), "a [31m b c");
        assert_eq!(clean(&"x".repeat(100), 10), format!("{}…", "x".repeat(10)));
        assert_eq!(sender("<who@x.org>"), "who@x.org");
        assert_eq!(sender("\"\" <who@x.org>"), "who@x.org");
        assert!(letters("not json").is_err());
        let many = format!("[{}]", (0..30).map(|n| format!(r#"{{"uid":{n},"from":"a@b.c","subject":"s{n}","date":"d","unread":true}}"#)).collect::<Vec<_>>().join(","));
        assert_eq!(describe("me", &letters(&many).unwrap(), false).lines().count(), 1 + LISTED, "one answer lists at most ten");
    }
}
