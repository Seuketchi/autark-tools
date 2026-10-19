//! Mail messages (RFC 5322 and MIME): reading what IMAP delivers into a subject, people, text and attachments,
//! and writing what SMTP sends. Headers with non-ASCII words (RFC 2047), quoted-printable and base64 bodies,
//! nested multipart messages and UTF-8, Latin-1 and Windows-1252 text are handled; other character sets are
//! shown as UTF-8 with replacement characters.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// A person or mailbox: `Ada Lovelace <ada@example.com>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub email: String,
}

impl Address {
    pub fn parse_list(value: &str) -> Vec<Address> {
        split_addresses(value).into_iter().filter_map(|part| Address::parse(&part)).collect()
    }

    fn parse(text: &str) -> Option<Address> {
        let text = text.trim();
        if let (Some(open), Some(close)) = (text.rfind('<'), text.rfind('>'))
            && open < close
        {
            let name = decode_words(text[..open].trim().trim_matches('"'));
            return Some(Address { name: name.trim().to_owned(), email: text[open + 1..close].trim().to_owned() });
        }
        text.contains('@').then(|| Address { name: String::new(), email: text.to_owned() })
    }

    /// The name if there is one, else the address.
    pub fn display(&self) -> &str {
        if self.name.is_empty() { &self.email } else { &self.name }
    }
}

/// Splits a header's address list at the commas that aren't inside quotes or angle brackets.
fn split_addresses(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut angle) = (false, false);
    for c in value.chars() {
        match c {
            '"' => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            ',' if !quoted && !angle => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// A message as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    /// As the header has it, such as `Mon, 19 Oct 2026 09:00:00 +0100`.
    pub date: String,
    pub message_id: String,
    pub in_reply_to: String,
    pub text: String,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// Headers (unfolded, names in lower case) and the body after them.
fn split_message(raw: &[u8]) -> (Vec<(String, String)>, &[u8]) {
    let (head, body) = match find(raw, b"\r\n\r\n").map(|at| (at, 4)).or_else(|| find(raw, b"\n\n").map(|at| (at, 2))) {
        Some((at, gap)) => (&raw[..at], &raw[at + gap..]),
        None => (raw, &raw[raw.len()..]),
    };
    let text = String::from_utf8_lossy(head);
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = headers.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    (headers, body)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()).unwrap_or_default()
}

/// `text/plain; charset=utf-8; name="a.txt"`: the type in lower case and the parameters.
fn content_type(value: &str) -> (String, Vec<(String, String)>) {
    let mut parts = value.split(';');
    let kind = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let params = parts.filter_map(|part| part.split_once('=')).map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().trim_matches('"').to_owned())).collect();
    (kind, params)
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

/// Reads a whole message.
pub fn parse(raw: &[u8]) -> Message {
    let (headers, body) = split_message(raw);
    let mut message = Message {
        from: Address::parse_list(header(&headers, "from")),
        to: Address::parse_list(header(&headers, "to")),
        cc: Address::parse_list(header(&headers, "cc")),
        subject: decode_words(header(&headers, "subject")),
        date: header(&headers, "date").to_owned(),
        message_id: header(&headers, "message-id").to_owned(),
        in_reply_to: header(&headers, "in-reply-to").to_owned(),
        ..Default::default()
    };
    part(&headers, body, &mut message);
    message
}

/// Adds one part (and its children) to `message`.
fn part(headers: &[(String, String)], body: &[u8], message: &mut Message) {
    let (kind, params) = content_type(if header(headers, "content-type").is_empty() { "text/plain" } else { header(headers, "content-type") });
    if let Some(boundary) = param(&params, "boundary").filter(|_| kind.starts_with("multipart/")) {
        for child in multipart(body, boundary) {
            let (child_headers, child_body) = split_message(child);
            part(&child_headers, child_body, message);
        }
        return;
    }
    let data = decode_body(body, header(headers, "content-transfer-encoding"));
    let disposition = header(headers, "content-disposition");
    let (disposition_kind, disposition_params) = content_type(disposition);
    let name = param(&disposition_params, "filename").or_else(|| param(&params, "name")).map(decode_words);
    let attached = disposition_kind == "attachment" || (name.is_some() && !kind.starts_with("text/")) || (!kind.starts_with("text/") && !kind.starts_with("multipart/"));
    if attached {
        message.attachments.push(Attachment { name: name.unwrap_or_else(|| "attachment".to_owned()), content_type: kind, data });
        return;
    }
    let text = decode_charset(&data, param(&params, "charset").unwrap_or("utf-8"));
    if kind == "text/html" {
        message.html.get_or_insert(text);
    } else if message.text.is_empty() {
        message.text = text;
    } else {
        message.text.push_str("\n\n");
        message.text.push_str(&text);
    }
}

/// The parts between `--boundary` lines.
fn multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let marker = format!("--{boundary}");
    let marker = marker.as_bytes();
    let mut parts = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut at = 0;
    while let Some(found) = find(&body[at..], marker) {
        let position = at + found;
        // A boundary starts a line.
        if position == 0 || body[position - 1] == b'\n' {
            starts.push(position);
        }
        at = position + marker.len();
    }
    for pair in starts.windows(2) {
        let (from, to) = (pair[0] + marker.len(), pair[1]);
        let mut section = &body[from..to];
        // The rest of the boundary line, and the line break before the next boundary.
        if let Some(line_end) = section.iter().position(|b| *b == b'\n') {
            section = &section[line_end + 1..];
        }
        section = section.strip_suffix(b"\r\n").or_else(|| section.strip_suffix(b"\n")).unwrap_or(section);
        parts.push(section);
    }
    parts
}

fn decode_body(body: &[u8], encoding: &str) -> Vec<u8> {
    match encoding.trim().to_ascii_lowercase().as_str() {
        "base64" => {
            let clean: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            STANDARD.decode(clean).unwrap_or_default()
        }
        "quoted-printable" => quoted_printable(body, false),
        _ => body.to_vec(),
    }
}

/// Quoted-printable. In a header word (`q`), an underscore is a space.
fn quoted_printable(input: &[u8], q: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut at = 0;
    while at < input.len() {
        match input[at] {
            b'=' => {
                // A soft line break.
                if input.get(at + 1) == Some(&b'\r') && input.get(at + 2) == Some(&b'\n') {
                    at += 3;
                } else if input.get(at + 1) == Some(&b'\n') {
                    at += 2;
                } else if let (Some(high), Some(low)) = (input.get(at + 1).and_then(|b| hex(*b)), input.get(at + 2).and_then(|b| hex(*b))) {
                    out.push(high * 16 + low);
                    at += 3;
                } else {
                    out.push(b'=');
                    at += 1;
                }
            }
            b'_' if q => {
                out.push(b' ');
                at += 1;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    out
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
}

fn decode_charset(data: &[u8], charset: &str) -> String {
    match charset.to_ascii_lowercase().as_str() {
        "iso-8859-1" | "latin1" | "us-ascii" | "ascii" => data.iter().map(|b| *b as char).collect(),
        "windows-1252" | "cp1252" => data.iter().map(|b| windows_1252(*b)).collect(),
        _ => String::from_utf8_lossy(data).into_owned(),
    }
}

fn windows_1252(byte: u8) -> char {
    match byte {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8a => 'Š',
        0x8b => '‹',
        0x8c => 'Œ',
        0x8e => 'Ž',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9a => 'š',
        0x9b => '›',
        0x9c => 'œ',
        0x9e => 'ž',
        0x9f => 'Ÿ',
        other => other as char,
    }
}

/// Header text with RFC 2047 encoded words (`=?utf-8?B?…?=`) decoded; whitespace between two encoded words
/// is dropped, as the standard says.
pub fn decode_words(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        let Some((decoded, used)) = word(&rest[start..]) else {
            out.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            last_was_word = false;
            continue;
        };
        if !(last_was_word && before.trim().is_empty()) {
            out.push_str(before);
        }
        out.push_str(&decoded);
        rest = &rest[start + used..];
        last_was_word = true;
    }
    out.push_str(rest);
    out
}

/// One encoded word at the start of `text`: its text and how many bytes it took.
fn word(text: &str) -> Option<(String, usize)> {
    let inner = text.strip_prefix("=?")?;
    let mut fields = inner.splitn(3, '?');
    let (charset, encoding, rest) = (fields.next()?, fields.next()?, fields.next()?);
    let end = rest.find("?=")?;
    let payload = &rest[..end];
    let bytes = match encoding.to_ascii_uppercase().as_str() {
        "B" => STANDARD.decode(payload).ok()?,
        "Q" => quoted_printable(payload.as_bytes(), true),
        _ => return None,
    };
    // "=?" charset "?" encoding "?" payload "?=".
    let consumed = 2 + charset.len() + 1 + encoding.len() + 1 + end + 2;
    let charset = charset.split('*').next().unwrap_or(charset);
    Some((decode_charset(&bytes, charset), consumed))
}

/// A message to send.
#[derive(Clone, Debug, Default)]
pub struct Outgoing {
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    pub text: String,
    pub in_reply_to: String,
    /// The message's ID, such as `<abc123@example.com>`; the caller makes it unique.
    pub message_id: String,
    pub date: String,
    pub attachments: Vec<Attachment>,
}

fn encode_word(text: &str) -> String {
    if text.is_ascii() && !text.contains(['\r', '\n']) {
        return text.to_owned();
    }
    format!("=?utf-8?B?{}?=", STANDARD.encode(text))
}

fn address_header(addresses: &[Address]) -> String {
    addresses
        .iter()
        .map(|a| if a.name.is_empty() { a.email.clone() } else { format!("{} <{}>", encode_word(&a.name).replace('"', ""), a.email) })
        .collect::<Vec<_>>()
        .join(", ")
}

fn wrap_base64(data: &[u8]) -> String {
    STANDARD.encode(data).as_bytes().chunks(76).map(|chunk| String::from_utf8_lossy(chunk).into_owned()).collect::<Vec<_>>().join("\r\n")
}

/// The message as RFC 5322 text, CRLF line ends. The body goes out as UTF-8 in base64 when it isn't plain
/// ASCII; attachments make it `multipart/mixed`. The boundary is derived from the message ID.
pub fn build(message: &Outgoing) -> String {
    let mut out = String::new();
    let mut header = |name: &str, value: &str| {
        if !value.is_empty() {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
    };
    header("From", &address_header(std::slice::from_ref(&message.from)));
    header("To", &address_header(&message.to));
    header("Cc", &address_header(&message.cc));
    header("Subject", &encode_word(&message.subject));
    header("Date", &message.date);
    header("Message-ID", &message.message_id);
    header("In-Reply-To", &message.in_reply_to);
    header("MIME-Version", "1.0");
    let body = |text: &str| -> (&'static str, String) {
        let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
        if text.is_ascii() { ("7bit", text) } else { ("base64", wrap_base64(text.as_bytes())) }
    };
    let (encoding, text) = body(&message.text);
    if message.attachments.is_empty() {
        out.push_str(&format!("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n{text}\r\n"));
        return out;
    }
    let boundary = format!("autark-{}", message.message_id.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>());
    out.push_str(&format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"));
    out.push_str(&format!("--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n{text}\r\n"));
    for attachment in &message.attachments {
        let name = encode_word(&attachment.name).replace('"', "'");
        out.push_str(&format!(
            "--{boundary}\r\nContent-Type: {}; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            attachment.content_type,
            wrap_base64(&attachment.data)
        ));
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_with_names_commas_and_encoded_words() {
        let list = Address::parse_list("\"Lovelace, Ada\" <ada@example.com>, =?utf-8?B?SsO8cmdlbg==?= <j@example.com>, bare@example.com");
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].name.as_str(), list[0].email.as_str()), ("Lovelace, Ada", "ada@example.com"));
        assert_eq!(list[1].name, "Jürgen");
        assert_eq!(list[2].display(), "bare@example.com");
    }

    #[test]
    fn encoded_words_join_and_decode() {
        assert_eq!(decode_words("=?utf-8?Q?Caf=C3=A9_menu?= =?utf-8?Q?_today?="), "Café menu today");
        assert_eq!(decode_words("Re: =?iso-8859-1?Q?caf=E9?= plain"), "Re: café plain");
        assert_eq!(decode_words("no words = here =?broken"), "no words = here =?broken");
    }

    #[test]
    fn a_multipart_message_with_an_attachment() {
        let raw = "From: Ada <ada@example.com>\r\nTo: me@example.com\r\nSubject: =?utf-8?B?SGVsbG8gw6k=?=\r\nDate: Mon, 19 Oct 2026 09:00:00 +0100\r\nMessage-ID: <1@example.com>\r\nContent-Type: multipart/mixed; boundary=\"outer\"\r\n\r\npreamble\r\n--outer\r\nContent-Type: multipart/alternative; boundary=inner\r\n\r\n--inner\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nCaf=C3=A9 =\r\nopen\r\n--inner\r\nContent-Type: text/html\r\n\r\n<p>Caf&eacute;</p>\r\n--inner--\r\n--outer\r\nContent-Type: application/pdf; name=\"a.pdf\"\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nUERG\r\n--outer--\r\n";
        let message = parse(raw.as_bytes());
        assert_eq!(message.subject, "Hello é");
        assert_eq!(message.from[0].name, "Ada");
        assert_eq!(message.text, "Café open");
        assert_eq!(message.html.as_deref(), Some("<p>Caf&eacute;</p>"));
        assert_eq!(message.attachments, vec![Attachment { name: "a.pdf".into(), content_type: "application/pdf".into(), data: b"PDF".to_vec() }]);
    }

    #[test]
    fn latin1_and_plain_messages() {
        let mut raw = b"Subject: x\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\r\ncaf".to_vec();
        raw.push(0xe9);
        assert_eq!(parse(&raw).text, "café");
        assert_eq!(parse(b"Subject: only headers\n\nbody\n").text, "body\n");
    }

    #[test]
    fn built_messages_read_back() {
        let out = Outgoing {
            from: Address { name: "Ada Lovelace".into(), email: "ada@example.com".into() },
            to: vec![Address { name: "Jürgen".into(), email: "j@example.com".into() }],
            subject: "Café plans".into(),
            text: "Meet at noon — café?\nBring notes.".into(),
            message_id: "<abc123@example.com>".into(),
            date: "Mon, 19 Oct 2026 09:00:00 +0000".into(),
            attachments: vec![Attachment { name: "n.txt".into(), content_type: "text/plain".into(), data: b"hello".to_vec() }],
            ..Default::default()
        };
        let text = build(&out);
        assert!(text.lines().all(|line| line.len() <= 998));
        let back = parse(text.as_bytes());
        assert_eq!(back.subject, "Café plans");
        assert_eq!(back.to[0].name, "Jürgen");
        assert_eq!(back.text.replace("\r\n", "\n").trim_end(), "Meet at noon — café?\nBring notes.");
        assert_eq!(back.attachments[0].data, b"hello");
        let plain = build(&Outgoing { text: "hi".into(), subject: "s".into(), attachments: Vec::new(), ..out });
        assert!(plain.contains("Content-Transfer-Encoding: 7bit") && !plain.contains("multipart"));
    }
}
