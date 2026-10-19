//! IMAP4rev1 (RFC 3501), enough for a mail app: folders, message lists, reading, flags, moving, deleting and
//! saving a copy of what was sent. Sign-in is a password (`LOGIN`) or an OAuth token (`XOAUTH2`, which Gmail
//! and Outlook use). The session works over any stream, so it is tested against a scripted server.

use crate::tls::{self, Io};

/// A folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub delimiter: String,
    pub flags: Vec<String>,
}

impl Folder {
    /// Whether the folder has the special use `\Sent`, `\Trash` and so on (RFC 6154).
    pub fn is(&self, use_flag: &str) -> bool {
        self.flags.iter().any(|flag| flag.eq_ignore_ascii_case(use_flag))
    }
}

/// What `SELECT` says about a folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mailbox {
    pub exists: u32,
    pub uid_validity: u32,
    pub uid_next: u32,
}

/// A message in a list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub uid: u32,
    pub flags: Vec<String>,
    pub size: u32,
    pub from: String,
    pub subject: String,
    pub date: String,
    pub message_id: String,
}

impl Summary {
    pub fn is_seen(&self) -> bool {
        self.flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"))
    }
}

/// One token of a server reply.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Atom(String),
    Text(Vec<u8>),
    List(Vec<Token>),
    Nil,
}

impl Token {
    fn text(&self) -> String {
        match self {
            Token::Atom(atom) => atom.clone(),
            Token::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            _ => String::new(),
        }
    }
}

/// An untagged reply (`* …`): its leading number if it has one, its keyword and the tokens after.
#[derive(Debug)]
struct Untagged {
    number: Option<u32>,
    keyword: String,
    rest: Vec<Token>,
}

pub struct Imap<S: Io> {
    stream: S,
    buffer: Vec<u8>,
    tag: u32,
    pub capabilities: Vec<String>,
}

/// Connects over TLS (port 993 by default) and reads the greeting.
pub fn connect(host: &str, port: u16) -> Result<Imap<tls::Stream>, String> {
    Imap::new(tls::connect(host, port)?)
}

impl<S: Io> Imap<S> {
    pub fn new(stream: S) -> Result<Self, String> {
        let mut imap = Imap { stream, buffer: Vec::new(), tag: 0, capabilities: Vec::new() };
        let greeting = imap.line()?;
        if !greeting.starts_with(b"* OK") && !greeting.starts_with(b"* PREAUTH") {
            return Err(format!("the server said: {}", String::from_utf8_lossy(&greeting).trim()));
        }
        imap.capabilities = imap.capability()?;
        Ok(imap)
    }

    /// One line from the server, without its line break.
    fn line(&mut self) -> Result<Vec<u8>, String> {
        loop {
            if let Some(at) = self.buffer.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buffer.drain(..=at).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
            let mut chunk = [0u8; 8192];
            let read = self.stream.read(&mut chunk).map_err(|err| format!("reading from the server: {err}"))?;
            if read == 0 {
                return Err("the server closed the connection".to_owned());
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    fn bytes(&mut self, count: usize) -> Result<Vec<u8>, String> {
        while self.buffer.len() < count {
            let mut chunk = [0u8; 8192];
            let read = self.stream.read(&mut chunk).map_err(|err| format!("reading from the server: {err}"))?;
            if read == 0 {
                return Err("the server closed the connection".to_owned());
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
        Ok(self.buffer.drain(..count).collect())
    }

    /// Sends a command and returns everything the server sent before its tagged answer, which must be `OK`.
    /// Literals (`{n}` and then n bytes) are kept inside the returned bytes.
    fn run(&mut self, command: &str) -> Result<Vec<u8>, String> {
        self.tag += 1;
        let tag = format!("A{}", self.tag);
        self.stream.write_all(format!("{tag} {command}\r\n").as_bytes()).and_then(|()| self.stream.flush()).map_err(|err| format!("writing to the server: {err}"))?;
        self.finish(&tag)
    }

    fn finish(&mut self, tag: &str) -> Result<Vec<u8>, String> {
        let mut raw = Vec::new();
        loop {
            let mut line = self.line()?;
            if let Some(status) = line.strip_prefix(format!("{tag} ").as_bytes()) {
                let status = String::from_utf8_lossy(status).into_owned();
                return if status.to_ascii_uppercase().starts_with("OK") { Ok(raw) } else { Err(format!("the server refused: {}", status.trim())) };
            }
            // A line ending in a literal's size is followed by that many bytes and then the rest of the line.
            loop {
                let Some(size) = literal_size(&line) else { break };
                raw.extend_from_slice(&line);
                raw.extend_from_slice(b"\r\n");
                let data = self.bytes(size)?;
                raw.extend_from_slice(&data);
                line = self.line()?;
            }
            raw.extend_from_slice(&line);
            raw.extend_from_slice(b"\r\n");
        }
    }

    pub fn capability(&mut self) -> Result<Vec<String>, String> {
        let raw = self.run("CAPABILITY")?;
        Ok(untagged(&raw)?.into_iter().filter(|reply| reply.keyword == "CAPABILITY").flat_map(|reply| reply.rest).map(|token| token.text().to_ascii_uppercase()).collect())
    }

    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|have| have == capability)
    }

    /// Signs in with a password.
    pub fn login(&mut self, user: &str, password: &str) -> Result<(), String> {
        self.run(&format!("LOGIN {} {}", quote(user), quote(password)))?;
        self.capabilities = self.capability()?;
        Ok(())
    }

    /// Signs in with an OAuth access token (Gmail, Outlook).
    pub fn login_oauth(&mut self, user: &str, token: &str) -> Result<(), String> {
        use base64::Engine;
        let initial = base64::engine::general_purpose::STANDARD.encode(format!("user={user}\x01auth=Bearer {token}\x01\x01"));
        self.tag += 1;
        let tag = format!("A{}", self.tag);
        self.stream.write_all(format!("{tag} AUTHENTICATE XOAUTH2 {initial}\r\n").as_bytes()).and_then(|()| self.stream.flush()).map_err(|err| format!("writing to the server: {err}"))?;
        // A failure comes as a continuation holding an error, which is answered with an empty line.
        loop {
            let line = self.line()?;
            if line.starts_with(b"+") {
                self.stream.write_all(b"\r\n").and_then(|()| self.stream.flush()).map_err(|err| format!("writing to the server: {err}"))?;
                continue;
            }
            if let Some(status) = line.strip_prefix(format!("{tag} ").as_bytes()) {
                let status = String::from_utf8_lossy(status).into_owned();
                if status.to_ascii_uppercase().starts_with("OK") {
                    break;
                }
                return Err(format!("sign-in refused: {}", status.trim()));
            }
        }
        self.capabilities = self.capability()?;
        Ok(())
    }

    /// Every folder.
    pub fn folders(&mut self) -> Result<Vec<Folder>, String> {
        let raw = self.run("LIST \"\" \"*\"")?;
        let mut folders = Vec::new();
        for reply in untagged(&raw)?.into_iter().filter(|reply| reply.keyword == "LIST") {
            if let [Token::List(flags), delimiter, name] = reply.rest.as_slice() {
                folders.push(Folder { name: decode_utf7(&name.text()), delimiter: delimiter.text(), flags: flags.iter().map(Token::text).collect() });
            }
        }
        Ok(folders)
    }

    /// Opens a folder (read-write).
    pub fn select(&mut self, folder: &str) -> Result<Mailbox, String> {
        let raw = self.run(&format!("SELECT {}", quote(&encode_utf7(folder))))?;
        let mut mailbox = Mailbox::default();
        for reply in untagged(&raw)? {
            match (reply.keyword.as_str(), reply.number) {
                ("EXISTS", Some(count)) => mailbox.exists = count,
                ("OK", _) => {
                    if let Some(Token::List(code)) = reply.rest.first() {
                        let value = code.get(1).map(Token::text).and_then(|text| text.parse().ok()).unwrap_or(0);
                        match code.first().map(Token::text).as_deref() {
                            Some("UIDVALIDITY") => mailbox.uid_validity = value,
                            Some("UIDNEXT") => mailbox.uid_next = value,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(mailbox)
    }

    /// The UIDs matching a search such as `ALL`, `UNSEEN` or `SINCE 1-Oct-2026`, oldest first.
    pub fn search(&mut self, criteria: &str) -> Result<Vec<u32>, String> {
        let raw = self.run(&format!("UID SEARCH {criteria}"))?;
        let mut uids: Vec<u32> = untagged(&raw)?.into_iter().filter(|reply| reply.keyword == "SEARCH").flat_map(|reply| reply.rest).filter_map(|token| token.text().parse().ok()).collect();
        uids.sort_unstable();
        Ok(uids)
    }

    /// The headers worth listing, for `uids`.
    pub fn summaries(&mut self, uids: &[u32]) -> Result<Vec<Summary>, String> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let raw = self.run(&format!("UID FETCH {set} (UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)])"))?;
        let mut found = Vec::new();
        for reply in untagged(&raw)?.into_iter().filter(|reply| reply.keyword == "FETCH") {
            let Some(Token::List(items)) = reply.rest.first() else { continue };
            let mut summary = Summary::default();
            let mut at = 0;
            while at + 1 < items.len() {
                let (name, value) = (items[at].text().to_ascii_uppercase(), &items[at + 1]);
                match name.as_str() {
                    "UID" => summary.uid = value.text().parse().unwrap_or(0),
                    "RFC822.SIZE" => summary.size = value.text().parse().unwrap_or(0),
                    "FLAGS" => {
                        if let Token::List(flags) = value {
                            summary.flags = flags.iter().map(Token::text).collect();
                        }
                    }
                    n if n.starts_with("BODY[") => {
                        let message = crate::mime::parse(match value {
                            Token::Text(bytes) => bytes,
                            _ => &[],
                        });
                        summary.from = message.from.first().map(|a| a.display().to_owned()).unwrap_or_default();
                        summary.subject = message.subject;
                        summary.date = message.date;
                        summary.message_id = message.message_id;
                    }
                    _ => {}
                }
                at += 2;
            }
            found.push(summary);
        }
        found.sort_by_key(|summary| summary.uid);
        Ok(found)
    }

    /// A whole message, without marking it read.
    pub fn fetch(&mut self, uid: u32) -> Result<Vec<u8>, String> {
        let raw = self.run(&format!("UID FETCH {uid} (BODY.PEEK[])"))?;
        for reply in untagged(&raw)?.into_iter().filter(|reply| reply.keyword == "FETCH") {
            let Some(Token::List(items)) = reply.rest.first() else { continue };
            for pair in items.chunks(2) {
                if let [name, Token::Text(bytes)] = pair
                    && name.text().to_ascii_uppercase().starts_with("BODY[")
                {
                    return Ok(bytes.clone());
                }
            }
        }
        Err(format!("message {uid} isn't there"))
    }

    /// Adds (`add`) or removes `flags` such as `\Seen` or `\Flagged`.
    pub fn flag(&mut self, uid: u32, flags: &[&str], add: bool) -> Result<(), String> {
        self.run(&format!("UID STORE {uid} {}FLAGS.SILENT ({})", if add { "+" } else { "-" }, flags.join(" "))).map(drop)
    }

    /// Moves a message to another folder: `MOVE` where the server has it, else a copy and a delete.
    pub fn move_to(&mut self, uid: u32, folder: &str) -> Result<(), String> {
        let target = quote(&encode_utf7(folder));
        if self.has("MOVE") {
            return self.run(&format!("UID MOVE {uid} {target}")).map(drop);
        }
        self.run(&format!("UID COPY {uid} {target}"))?;
        self.flag(uid, &["\\Deleted"], true)?;
        self.run(&format!("UID EXPUNGE {uid}")).or_else(|_| self.run("EXPUNGE")).map(drop)
    }

    /// Saves `message` in `folder`, marked read, as a copy of what was sent.
    pub fn append(&mut self, folder: &str, message: &str) -> Result<(), String> {
        self.tag += 1;
        let tag = format!("A{}", self.tag);
        let head = format!("{tag} APPEND {} (\\Seen) {{{}}}\r\n", quote(&encode_utf7(folder)), message.len());
        self.stream.write_all(head.as_bytes()).and_then(|()| self.stream.flush()).map_err(|err| format!("writing to the server: {err}"))?;
        let go = self.line()?;
        if !go.starts_with(b"+") {
            return Err(format!("the server refused the message: {}", String::from_utf8_lossy(&go).trim()));
        }
        self.stream.write_all(message.as_bytes()).and_then(|()| self.stream.write_all(b"\r\n")).and_then(|()| self.stream.flush()).map_err(|err| format!("writing to the server: {err}"))?;
        self.finish(&tag).map(drop)
    }

    pub fn logout(&mut self) {
        let _ = self.run("LOGOUT");
    }
}

/// `{123}` at the end of a line: the literal's size.
fn literal_size(line: &[u8]) -> Option<usize> {
    let inner = line.strip_suffix(b"}")?;
    let open = inner.iter().rposition(|b| *b == b'{')?;
    let digits = std::str::from_utf8(&inner[open + 1..]).ok()?.trim_end_matches('+');
    digits.parse().ok()
}

/// A string as an IMAP quoted string.
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The server's untagged replies in `raw`.
fn untagged(raw: &[u8]) -> Result<Vec<Untagged>, String> {
    let mut replies = Vec::new();
    let mut at = 0;
    while at < raw.len() {
        let Some(rest) = raw[at..].strip_prefix(b"* ") else {
            // Anything else (a continuation) is skipped to the end of its line.
            at += raw[at..].windows(2).position(|pair| pair == b"\r\n").map_or(raw.len() - at, |end| end + 2);
            continue;
        };
        let _ = rest;
        at += 2;
        let mut cursor = Cursor { raw, at };
        let mut tokens = cursor.tokens(false)?;
        at = cursor.at;
        if tokens.is_empty() {
            continue;
        }
        let number = tokens.first().and_then(|token| if let Token::Atom(atom) = token { atom.parse().ok() } else { None });
        if number.is_some() {
            tokens.remove(0);
        }
        let keyword = if tokens.is_empty() { String::new() } else { tokens.remove(0).text().to_ascii_uppercase() };
        replies.push(Untagged { number, keyword, rest: tokens });
    }
    Ok(replies)
}

struct Cursor<'a> {
    raw: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    /// Tokens up to the end of the line, or up to the `)` that closes the list when `in_list`.
    fn tokens(&mut self, in_list: bool) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            match self.raw.get(self.at) {
                None => return Ok(tokens),
                Some(b' ') => self.at += 1,
                Some(b'\r') => {
                    self.at += if self.raw.get(self.at + 1) == Some(&b'\n') { 2 } else { 1 };
                    return Ok(tokens);
                }
                Some(b'\n') => {
                    self.at += 1;
                    return Ok(tokens);
                }
                Some(b')') if in_list => {
                    self.at += 1;
                    return Ok(tokens);
                }
                Some(b'(') => {
                    self.at += 1;
                    tokens.push(Token::List(self.tokens(true)?));
                }
                Some(b'"') => {
                    self.at += 1;
                    let mut text = Vec::new();
                    loop {
                        match self.raw.get(self.at) {
                            None => return Err("a quoted string never ends".to_owned()),
                            Some(b'\\') => {
                                text.extend(self.raw.get(self.at + 1));
                                self.at += 2;
                            }
                            Some(b'"') => {
                                self.at += 1;
                                break;
                            }
                            Some(byte) => {
                                text.push(*byte);
                                self.at += 1;
                            }
                        }
                    }
                    tokens.push(Token::Text(text));
                }
                Some(b'{') => {
                    let end = self.raw[self.at..].iter().position(|b| *b == b'}').ok_or("a literal's size never ends")?;
                    let size: usize = std::str::from_utf8(&self.raw[self.at + 1..self.at + end]).ok().and_then(|digits| digits.trim_end_matches('+').parse().ok()).ok_or("a literal has no size")?;
                    // Past "}\r\n".
                    let start = self.at + end + 3;
                    let data = self.raw.get(start..start + size).ok_or("a literal is cut short")?;
                    tokens.push(Token::Text(data.to_vec()));
                    self.at = start + size;
                }
                Some(b'[') => {
                    // A response code such as [UIDVALIDITY 3] reads as a list.
                    self.at += 1;
                    let mut inner = Vec::new();
                    let mut word = String::new();
                    while let Some(byte) = self.raw.get(self.at) {
                        self.at += 1;
                        match byte {
                            b']' => break,
                            b' ' => {
                                if !word.is_empty() {
                                    inner.push(Token::Atom(std::mem::take(&mut word)));
                                }
                            }
                            other => word.push(*other as char),
                        }
                    }
                    if !word.is_empty() {
                        inner.push(Token::Atom(word));
                    }
                    tokens.push(Token::List(inner));
                }
                Some(_) => {
                    let start = self.at;
                    while let Some(byte) = self.raw.get(self.at) {
                        if matches!(byte, b' ' | b'\r' | b'\n' | b'(' | b')' | b'[') {
                            break;
                        }
                        // A section such as BODY[HEADER.FIELDS (FROM)] belongs to its atom.
                        self.at += 1;
                    }
                    let mut atom = String::from_utf8_lossy(&self.raw[start..self.at]).into_owned();
                    if self.raw.get(self.at) == Some(&b'[') {
                        let close = self.raw[self.at..].iter().position(|b| *b == b']').ok_or("a section never ends")?;
                        atom.push_str(&String::from_utf8_lossy(&self.raw[self.at..=self.at + close]));
                        self.at += close + 1;
                    }
                    tokens.push(if atom.eq_ignore_ascii_case("NIL") { Token::Nil } else { Token::Atom(atom) });
                }
            }
        }
    }
}

/// A folder name from IMAP's modified UTF-7 (`&AOk-` for é).
fn decode_utf7(name: &str) -> String {
    use base64::Engine;
    let mut out = String::new();
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &rest[start + 1..start + end];
        if inner.is_empty() {
            out.push('&');
        } else {
            let standard = inner.replace(',', "/");
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD.decode(standard.trim_end_matches('=')).unwrap_or_default();
            let units: Vec<u16> = bytes.chunks(2).filter(|pair| pair.len() == 2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
            out.push_str(&String::from_utf16_lossy(&units));
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

fn encode_utf7(name: &str) -> String {
    use base64::Engine;
    let mut out = String::new();
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|unit| unit.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes).replace('/', ","));
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut units = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut units));
        }
    }
    flush(&mut pending, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    /// A server that has said everything in advance, and records what the client wrote.
    struct Scripted {
        replies: std::io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.replies.read(buf)
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn session(script: &str) -> Imap<Scripted> {
        Imap::new(Scripted { replies: std::io::Cursor::new(script.as_bytes().to_vec()), written: Vec::new() }).unwrap()
    }

    fn sent(imap: &Imap<Scripted>) -> String {
        String::from_utf8_lossy(&imap.stream.written).into_owned()
    }

    const OPEN: &str = "* OK ready\r\n* CAPABILITY IMAP4rev1 MOVE UIDPLUS\r\nA1 OK done\r\n";

    #[test]
    fn signing_in_and_listing_folders() {
        let mut imap = session(&format!("{OPEN}A2 OK logged in\r\n* CAPABILITY IMAP4rev1 MOVE\r\nA3 OK done\r\n* LIST (\\HasNoChildren \\Sent) \"/\" \"Sent Items\"\r\n* LIST (\\HasNoChildren) \"/\" \"Caf&AOk-\"\r\nA4 OK listed\r\n"));
        assert!(imap.has("MOVE"));
        imap.login("ada@example.com", "p\"w").unwrap();
        let folders = imap.folders().unwrap();
        assert_eq!(folders.len(), 2);
        assert!(folders[0].is("\\Sent"));
        assert_eq!(folders[1].name, "Café");
        assert!(sent(&imap).contains("A2 LOGIN \"ada@example.com\" \"p\\\"w\""));
    }

    #[test]
    fn selecting_searching_and_listing_messages() {
        let header = "From: Ada <ada@example.com>\r\nSubject: =?utf-8?Q?Caf=C3=A9?=\r\nDate: Mon, 19 Oct 2026 09:00:00 +0100\r\nMessage-ID: <1@x>\r\n\r\n";
        let script = format!(
            "{OPEN}* 3 EXISTS\r\n* OK [UIDVALIDITY 77] ok\r\n* OK [UIDNEXT 9] ok\r\nA2 OK [READ-WRITE] selected\r\n* SEARCH 8 4 6\r\nA3 OK searched\r\n* 1 FETCH (UID 4 FLAGS (\\Seen) RFC822.SIZE 1200 BODY[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)] {{{}}}\r\n{header})\r\nA4 OK fetched\r\n",
            header.len()
        );
        let mut imap = session(&script);
        let mailbox = imap.select("INBOX").unwrap();
        assert_eq!(mailbox, Mailbox { exists: 3, uid_validity: 77, uid_next: 9 });
        assert_eq!(imap.search("ALL").unwrap(), vec![4, 6, 8]);
        let list = imap.summaries(&[4]).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].uid, list[0].size, list[0].is_seen()), (4, 1200, true));
        assert_eq!((list[0].from.as_str(), list[0].subject.as_str()), ("Ada", "Café"));
        assert!(sent(&imap).contains("A4 UID FETCH 4 (UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)])"));
    }

    #[test]
    fn a_literal_can_hold_what_looks_like_protocol() {
        let body = "Subject: tricky\r\n\r\n* 99 EXISTS\r\nA9 OK fake\r\n";
        let mut imap = session(&format!("{OPEN}* 1 FETCH (UID 5 BODY[] {{{}}}\r\n{body})\r\nA2 OK done\r\n", body.len()));
        assert_eq!(imap.fetch(5).unwrap(), body.as_bytes());
    }

    #[test]
    fn flags_moves_and_appends() {
        let mut imap = session(&format!("{OPEN}A2 OK stored\r\nA3 OK moved\r\n+ go ahead\r\nA4 OK appended\r\n"));
        imap.flag(4, &["\\Seen"], true).unwrap();
        imap.move_to(4, "Trash").unwrap();
        imap.append("Sent Items", "Subject: hi\r\n\r\nthere").unwrap();
        let written = sent(&imap);
        assert!(written.contains("A2 UID STORE 4 +FLAGS.SILENT (\\Seen)"));
        assert!(written.contains("A3 UID MOVE 4 \"Trash\""));
        assert!(written.contains("A4 APPEND \"Sent Items\" (\\Seen) {"));
    }

    #[test]
    fn refusals_say_why() {
        let mut imap = session(&format!("{OPEN}A2 NO [AUTHENTICATIONFAILED] Invalid credentials\r\n"));
        assert!(imap.login("a", "b").unwrap_err().contains("Invalid credentials"));
    }

    #[test]
    fn folder_names_in_modified_utf7() {
        assert_eq!(decode_utf7("Caf&AOk-/&-x"), "Café/&x");
        assert_eq!(encode_utf7("Café/&x"), "Caf&AOk-/&-x");
        assert_eq!(decode_utf7(&encode_utf7("日本語")), "日本語");
    }
}
