//! vCard (RFC 6350, and the 3.0 that Google and Apple still send): a contact's name, addresses, numbers and
//! organisation, with the properties this code doesn't read kept for writing back.

/// A contact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contact {
    pub uid: String,
    /// The display name (`FN`).
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organisation: String,
    pub note: String,
    /// Lines not read here (photos, addresses, birthdays, vendor properties), as written.
    pub extra: Vec<String>,
}

fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
        } else if !line.is_empty() {
            lines.push(line.to_owned());
        }
    }
    lines
}

fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\r', "").replace('\n', "\\n")
}

/// The contacts in `text`; a file can hold several cards.
pub fn parse(text: &str) -> Vec<Contact> {
    let mut contacts = Vec::new();
    let mut current: Option<Contact> = None;
    for raw in unfold(text) {
        let Some((head, value)) = raw.split_once(':') else { continue };
        let name = head.split(';').next().unwrap_or_default().to_ascii_uppercase();
        // Grouped properties such as "item1.EMAIL" lose their group for matching.
        let name = name.rsplit('.').next().unwrap_or_default().to_owned();
        match (name.as_str(), &mut current) {
            ("BEGIN", _) if value.eq_ignore_ascii_case("VCARD") => current = Some(Contact::default()),
            ("END", _) if value.eq_ignore_ascii_case("VCARD") => contacts.extend(current.take()),
            ("VERSION", Some(_)) => {}
            ("UID", Some(contact)) => contact.uid = value.to_owned(),
            ("FN", Some(contact)) => contact.name = unescape(value),
            ("EMAIL", Some(contact)) => contact.emails.push(unescape(value)),
            ("TEL", Some(contact)) => contact.phones.push(unescape(value)),
            ("ORG", Some(contact)) => contact.organisation = value.split(';').map(unescape).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(", "),
            ("NOTE", Some(contact)) => contact.note = unescape(value),
            (_, Some(contact)) => contact.extra.push(raw.clone()),
            _ => {}
        }
    }
    contacts
}

/// Writes `contact` as a vCard 3.0 with CRLF line ends, folded at 75 bytes.
pub fn write(contact: &Contact) -> String {
    let mut lines = vec!["BEGIN:VCARD".to_owned(), "VERSION:3.0".to_owned(), format!("UID:{}", contact.uid), format!("FN:{}", escape(&contact.name))];
    // `N` is required by 3.0; a display name split at its last space is the best guess.
    let (given, family) = contact.name.rsplit_once(' ').unwrap_or((&contact.name, ""));
    if !contact.extra.iter().any(|line| line.to_ascii_uppercase().starts_with("N:") || line.to_ascii_uppercase().starts_with("N;")) {
        lines.push(format!("N:{};{};;;", escape(family), escape(given)));
    }
    lines.extend(contact.emails.iter().map(|email| format!("EMAIL:{}", escape(email))));
    lines.extend(contact.phones.iter().map(|phone| format!("TEL:{}", escape(phone))));
    if !contact.organisation.is_empty() {
        lines.push(format!("ORG:{}", escape(&contact.organisation)));
    }
    if !contact.note.is_empty() {
        lines.push(format!("NOTE:{}", escape(&contact.note)));
    }
    lines.extend(contact.extra.iter().cloned());
    lines.push("END:VCARD".to_owned());
    let mut out = String::new();
    for line in lines {
        let mut width = 0;
        for c in line.chars() {
            if width + c.len_utf8() > 75 {
                out.push_str("\r\n ");
                width = 1;
            }
            out.push(c);
            width += c.len_utf8();
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:u1\r\nFN:Ada Lovelace\r\nN:Lovelace;Ada;;;\r\nitem1.EMAIL;TYPE=INTERNET:ada@example.com\r\nEMAIL:ada@work.example\r\nTEL;TYPE=CELL:+44 20 7946 0000\r\nORG:Analytical\\, Ltd;Engines\r\nNOTE:Met at\\nthe fair\r\nBDAY:18151210\r\nPHOTO;ENCODING=b;TYPE=JPEG:AAAA\r\n BBBB\r\nEND:VCARD\r\n";

    #[test]
    fn reads_a_card() {
        let contacts = parse(CARD);
        assert_eq!(contacts.len(), 1);
        let ada = &contacts[0];
        assert_eq!((ada.uid.as_str(), ada.name.as_str()), ("u1", "Ada Lovelace"));
        assert_eq!(ada.emails, vec!["ada@example.com", "ada@work.example"]);
        assert_eq!(ada.phones, vec!["+44 20 7946 0000"]);
        assert_eq!(ada.organisation, "Analytical, Ltd, Engines");
        assert_eq!(ada.note, "Met at\nthe fair");
        assert_eq!(ada.extra, vec!["N:Lovelace;Ada;;;", "BDAY:18151210", "PHOTO;ENCODING=b;TYPE=JPEG:AAAABBBB"]);
    }

    #[test]
    fn writing_round_trips_and_adds_a_name() {
        let ada = &parse(CARD)[0];
        assert_eq!(parse(&write(ada))[0], *ada);
        let new = Contact { uid: "n".into(), name: "Grace Brewster Hopper".into(), emails: vec!["g@example.com".into()], ..Default::default() };
        let text = write(&new);
        assert!(text.contains("N:Hopper;Grace Brewster;;;") && text.contains("FN:Grace Brewster Hopper"));
        assert!(text.lines().all(|line| line.len() <= 75));
    }

    #[test]
    fn several_cards_in_one_file() {
        assert_eq!(parse(&format!("{CARD}{CARD}")).len(), 2);
    }
}
