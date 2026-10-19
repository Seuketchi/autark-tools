//! What the mail app shows and writes, worked out from messages: folder order, dates, replies and quoted text,
//! and plain text from HTML mail. Nothing here touches the network or a window.

use autark_pim::imap::Folder;
use autark_pim::mime::{Address, Message};
use chrono::{DateTime, Datelike, Local, NaiveDate};

/// How soon a folder comes in the list: Inbox, Drafts, Sent, Archive, Spam, Trash, then the rest by name.
fn rank(folder: &Folder) -> u8 {
    let name = folder.name.to_lowercase();
    let last = name.rsplit(['/', '.']).next().unwrap_or(&name);
    if name == "inbox" {
        0
    } else if folder.is("\\Drafts") || last == "drafts" {
        1
    } else if folder.is("\\Sent") || last == "sent" || last == "sent items" || last == "sent mail" {
        2
    } else if folder.is("\\Archive") || folder.is("\\All") || last == "archive" || last == "all mail" {
        3
    } else if folder.is("\\Junk") || last == "spam" || last == "junk" || last == "junk email" {
        4
    } else if folder.is("\\Trash") || last == "trash" || last == "deleted items" || last == "bin" {
        5
    } else {
        6
    }
}

/// Folders you can open (not `\Noselect` containers), in the order a mail app lists them.
pub fn ordered(mut folders: Vec<Folder>) -> Vec<Folder> {
    folders.retain(|folder| !folder.is("\\Noselect") && !folder.is("\\NonExistent"));
    folders.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    folders
}

/// The name a folder is shown under: the last part of its path.
pub fn folder_title(folder: &Folder) -> String {
    let delimiter = folder.delimiter.chars().next().unwrap_or('/');
    let name = folder.name.rsplit(delimiter).next().unwrap_or(&folder.name);
    if folder.name.eq_ignore_ascii_case("inbox") { "Inbox".to_owned() } else { name.to_owned() }
}

/// Where deleted mail goes: the folder marked `\Trash`, else one with a usual name.
pub fn trash_of(folders: &[Folder]) -> Option<&Folder> {
    folders.iter().find(|folder| folder.is("\\Trash")).or_else(|| folders.iter().find(|folder| rank(folder) == 5))
}

pub fn sent_of(folders: &[Folder]) -> Option<&Folder> {
    folders.iter().find(|folder| folder.is("\\Sent")).or_else(|| folders.iter().find(|folder| rank(folder) == 2))
}

/// A message's date for a list: the time today, the day and month this year, else the full date.
pub fn short_date(header: &str, today: NaiveDate) -> String {
    let Ok(moment) = DateTime::parse_from_rfc2822(header.trim()).or_else(|_| DateTime::parse_from_rfc2822(&header.replace(" (UTC)", "").replace(" (GMT)", ""))) else { return header.chars().take(16).collect() };
    let local = moment.with_timezone(&Local);
    let day = local.date_naive();
    if day == today {
        local.format("%H:%M").to_string()
    } else if day.year() == today.year() {
        local.format("%-d %b").to_string()
    } else {
        local.format("%-d %b %Y").to_string()
    }
}

/// The date written out for the message view.
pub fn long_date(header: &str) -> String {
    DateTime::parse_from_rfc2822(header.trim()).map_or_else(|_| header.to_owned(), |moment| moment.with_timezone(&Local).format("%A %-d %B %Y, %H:%M").to_string())
}

/// `Re: ` once, whatever the subject already has.
pub fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed.len() >= 3 && trimmed[..3].eq_ignore_ascii_case("re:") { trimmed.to_owned() } else { format!("Re: {trimmed}") }
}

pub fn forward_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed.len() >= 4 && trimmed[..4].eq_ignore_ascii_case("fwd:") { trimmed.to_owned() } else { format!("Fwd: {trimmed}") }
}

/// The text to start a reply with: the original, each line quoted with `> `.
pub fn quote(message: &Message) -> String {
    let who = message.from.first().map_or("someone", |from| from.display());
    let quoted: Vec<String> = body_text(message).lines().map(|line| if line.starts_with('>') { format!(">{line}") } else { format!("> {line}") }).collect();
    format!("\n\nOn {}, {who} wrote:\n{}\n", long_date(&message.date), quoted.join("\n"))
}

/// Who a reply goes to (everyone but you, for reply-all): the sender, and with `all` the others on To and Cc.
pub fn reply_to(message: &Message, mine: &str, all: bool) -> (Vec<Address>, Vec<Address>) {
    let mine = mine.to_lowercase();
    let not_me = |address: &&Address| address.email.to_lowercase() != mine;
    let to: Vec<Address> = message.from.iter().filter(not_me).cloned().collect();
    let cc: Vec<Address> = if all { message.to.iter().chain(&message.cc).filter(not_me).filter(|address| !to.iter().any(|sender| sender.email.eq_ignore_ascii_case(&address.email))).cloned().collect() } else { Vec::new() };
    (if to.is_empty() { message.from.clone() } else { to }, cc)
}

/// The message as text: the plain part, else the HTML part with its markup taken out.
pub fn body_text(message: &Message) -> String {
    if !message.text.trim().is_empty() {
        return message.text.replace("\r\n", "\n");
    }
    message.html.as_deref().map(strip_html).unwrap_or_default()
}

/// HTML as plain text: scripts, styles and tags dropped, block ends as new lines, entities decoded.
pub fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('>').map_or(rest.len(), |at| start + at + 1);
        let tag = rest[start..end].to_lowercase();
        rest = &rest[end..];
        let name: String = tag.trim_start_matches(['<', '/']).chars().take_while(char::is_ascii_alphanumeric).collect();
        if (name == "script" || name == "style" || name == "head") && !tag.starts_with("</") {
            let close = format!("</{name}");
            rest = rest.to_lowercase().find(&close).map_or("", |at| &rest[at..]);
            continue;
        }
        if ["p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "table", "blockquote"].contains(&name.as_str()) {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    out.push_str(rest);
    let decoded = decode_entities(&out);
    let lines: Vec<String> = decoded.lines().map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    // No more than one blank line in a row.
    let mut kept: Vec<&str> = Vec::new();
    for line in &lines {
        if !(line.is_empty() && kept.last().is_none_or(|last| last.is_empty())) {
            kept.push(line);
        }
    }
    kept.join("\n").trim().to_owned()
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|end| *end <= 8) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "eacute" => Some('é'),
            "copy" => Some('©'),
            "hellip" => Some('…'),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            _ => name.strip_prefix("#x").and_then(|hex| u32::from_str_radix(hex, 16).ok()).or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok())).and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `a@x.org, "B C" <b@x.org>` as addresses; entries without an `@` are dropped.
pub fn addresses(text: &str) -> Vec<Address> {
    Address::parse_list(text)
}

/// A message ID for what we send, from a unique string and the sender's domain.
pub fn message_id(unique: &str, email: &str) -> String {
    let domain = email.rsplit('@').next().filter(|domain| !domain.is_empty()).unwrap_or("autark.local");
    format!("<{unique}@{domain}>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, flags: &[&str]) -> Folder {
        Folder { name: name.into(), delimiter: "/".into(), flags: flags.iter().map(|flag| (*flag).to_owned()).collect() }
    }

    #[test]
    fn folders_are_ordered_and_found() {
        let list = ordered(vec![folder("Zebra", &[]), folder("[Gmail]/Trash", &["\\Trash"]), folder("[Gmail]", &["\\Noselect"]), folder("Sent Items", &[]), folder("INBOX", &[]), folder("Drafts", &[]), folder("Apples", &[])]);
        let names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["INBOX", "Drafts", "Sent Items", "[Gmail]/Trash", "Apples", "Zebra"]);
        assert_eq!((folder_title(&list[3]).as_str(), folder_title(&list[0]).as_str()), ("Trash", "Inbox"));
        assert_eq!(trash_of(&list).map(|f| f.name.as_str()), Some("[Gmail]/Trash"));
        assert_eq!(sent_of(&list).map(|f| f.name.as_str()), Some("Sent Items"));
    }

    #[test]
    fn dates_read_short_and_long() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let on = |header: &str| short_date(header, today);
        assert_eq!(on("Mon, 19 Oct 2026 12:00:00 +0000").len(), 5);
        assert!(on("Sat, 3 Oct 2026 12:00:00 +0000").ends_with("Oct") && on("Sat, 3 Oct 2026 12:00:00 +0000").len() <= 6);
        assert!(on("Fri, 3 Oct 2025 12:00:00 +0000").ends_with("2025"));
        assert_eq!(on("not a date"), "not a date");
        assert!(long_date("Mon, 19 Oct 2026 12:00:00 +0000").contains("October 2026"));
    }

    #[test]
    fn replies_quote_and_address() {
        assert_eq!((reply_subject("Plans").as_str(), reply_subject("RE: Plans").as_str(), forward_subject("Fwd: x").as_str()), ("Re: Plans", "RE: Plans", "Fwd: x"));
        let message = Message {
            from: vec![Address { name: "Ada".into(), email: "ada@x.org".into() }],
            to: vec![Address { name: String::new(), email: "me@x.org".into() }, Address { name: String::new(), email: "sam@x.org".into() }],
            cc: vec![Address { name: String::new(), email: "ADA@x.org".into() }],
            text: "Hello\n> earlier\nBye".into(),
            date: "Mon, 19 Oct 2026 12:00:00 +0000".into(),
            ..Default::default()
        };
        let text = quote(&message);
        assert!(text.contains("Ada wrote:") && text.contains("> Hello") && text.contains(">> earlier"));
        let (to, cc) = reply_to(&message, "me@x.org", true);
        assert_eq!((to.len(), cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>()), (1, vec!["sam@x.org"]));
        assert!(reply_to(&message, "me@x.org", false).1.is_empty());
        assert_eq!(reply_to(&message, "ada@x.org", false).0[0].email, "ada@x.org");
    }

    #[test]
    fn html_mail_reads_as_text() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body><p>Hi&nbsp;there &amp; welcome</p><p>Line<br>two &#233; &#x41;</p><script>bad()</script><ul><li>one</li><li>two</li></ul><p></p><p></p><p>End</p></body></html>";
        assert_eq!(strip_html(html), "Hi there & welcome\n\nLine\ntwo é A\n\none\n\ntwo\n\nEnd");
        let only_html = Message { html: Some("<p>Only</p>".into()), ..Default::default() };
        assert_eq!(body_text(&only_html), "Only");
        assert_eq!(body_text(&Message { text: "plain\r\ntext".into(), html: Some("<p>x</p>".into()), ..Default::default() }), "plain\ntext");
        assert_eq!(message_id("abc", "ada@x.org"), "<abc@x.org>");
    }
}
