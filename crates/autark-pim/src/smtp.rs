//! SMTP submission (RFC 6409, 5321): port 465 over TLS or port 587 with STARTTLS, signing in with a password
//! (`AUTH PLAIN`) or an OAuth token (`XOAUTH2`), then sending one message to its recipients.

use std::io::{Read, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::tls::{self, Io};

/// How to sign in.
#[derive(Clone, Debug)]
pub enum Login {
    Password { user: String, password: String },
    OAuth { user: String, token: String },
}

/// Sends `message` (as `mime::build` writes it) from `from` to `recipients` through `host:port`. Port 465 is
/// TLS from the start; any other port starts plain and must offer STARTTLS, so the password never goes in the clear.
pub fn send(host: &str, port: u16, login: &Login, from: &str, recipients: &[String], message: &str) -> Result<(), String> {
    if port == 465 {
        return session(tls::connect(host, port)?, host, login, from, recipients, message);
    }
    let mut plain = tls::tcp(host, port)?;
    expect(&mut plain, 220)?;
    let features = command(&mut plain, &format!("EHLO {}", local_name()), 250)?;
    if !features.to_ascii_uppercase().contains("STARTTLS") {
        return Err(format!("{host} doesn't offer STARTTLS, so nothing is sent"));
    }
    command(&mut plain, "STARTTLS", 220)?;
    let secure = tls::wrap(host, plain)?;
    // The TLS session starts with a fresh EHLO and no greeting.
    session_after_greeting(secure, login, from, recipients, message)
}

fn local_name() -> String {
    "localhost".to_owned()
}

fn session<S: Io>(mut stream: S, _host: &str, login: &Login, from: &str, recipients: &[String], message: &str) -> Result<(), String> {
    expect(&mut stream, 220)?;
    session_after_greeting(stream, login, from, recipients, message)
}

fn session_after_greeting<S: Io>(mut stream: S, login: &Login, from: &str, recipients: &[String], message: &str) -> Result<(), String> {
    command(&mut stream, &format!("EHLO {}", local_name()), 250)?;
    match login {
        Login::Password { user, password } => {
            let plain = STANDARD.encode(format!("\0{user}\0{password}"));
            command(&mut stream, &format!("AUTH PLAIN {plain}"), 235)?;
        }
        Login::OAuth { user, token } => {
            let initial = STANDARD.encode(format!("user={user}\x01auth=Bearer {token}\x01\x01"));
            command(&mut stream, &format!("AUTH XOAUTH2 {initial}"), 235)?;
        }
    }
    command(&mut stream, &format!("MAIL FROM:<{}>", clean(from)?), 250)?;
    for recipient in recipients {
        command(&mut stream, &format!("RCPT TO:<{}>", clean(recipient)?), 250)?;
    }
    command(&mut stream, "DATA", 354)?;
    stream.write_all(dot_stuffed(message).as_bytes()).and_then(|()| stream.write_all(b"\r\n.\r\n")).and_then(|()| stream.flush()).map_err(|err| format!("sending the message: {err}"))?;
    expect(&mut stream, 250)?;
    let _ = command(&mut stream, "QUIT", 221);
    Ok(())
}

/// An address with nothing in it that could add commands.
fn clean(address: &str) -> Result<&str, String> {
    let address = address.trim();
    if address.is_empty() || address.contains(['<', '>', '\r', '\n', ' ']) {
        return Err(format!("“{address}” isn't an address"));
    }
    Ok(address)
}

/// The message with lines that start with a dot doubled, and a CRLF end so the closing dot stands alone.
fn dot_stuffed(message: &str) -> String {
    let message = message.replace("\r\n", "\n").replace('\n', "\r\n");
    let message = message.strip_suffix("\r\n").unwrap_or(&message);
    let mut out = String::with_capacity(message.len() + 16);
    for (n, line) in message.split("\r\n").enumerate() {
        if n > 0 {
            out.push_str("\r\n");
        }
        if line.starts_with('.') {
            out.push('.');
        }
        out.push_str(line);
    }
    out
}

fn command<S: Read + Write>(stream: &mut S, line: &str, expected: u16) -> Result<String, String> {
    stream.write_all(line.as_bytes()).and_then(|()| stream.write_all(b"\r\n")).and_then(|()| stream.flush()).map_err(|err| format!("writing to the server: {err}"))?;
    expect(stream, expected)
}

/// Reads one reply (possibly several lines) and checks its code. Reads a byte at a time so nothing meant for a
/// later step, or for the TLS layer after STARTTLS, is swallowed.
fn expect<S: Read>(stream: &mut S, expected: u16) -> Result<String, String> {
    let mut reply = String::new();
    loop {
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            if stream.read(&mut byte).map_err(|err| format!("reading from the server: {err}"))? == 0 {
                return Err("the server closed the connection".to_owned());
            }
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
        }
        let line = String::from_utf8_lossy(&line).trim_end_matches('\r').to_owned();
        reply.push_str(&line);
        reply.push('\n');
        // "250-more" continues; "250 last" ends the reply.
        if line.as_bytes().get(3) != Some(&b'-') {
            let code: u16 = line.get(..3).and_then(|code| code.parse().ok()).ok_or_else(|| format!("the server said: {line}"))?;
            return if code == expected { Ok(reply) } else { Err(format!("the server said: {}", line.trim())) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn server(script: &str) -> Scripted {
        Scripted { replies: std::io::Cursor::new(script.as_bytes().to_vec()), written: Vec::new() }
    }

    #[test]
    fn a_message_goes_through_in_order() {
        let mut stream = server("250-smtp.example\r\n250 AUTH PLAIN XOAUTH2\r\n235 ok\r\n250 ok\r\n250 ok\r\n250 ok\r\n354 go\r\n250 queued\r\n221 bye\r\n");
        let login = Login::Password { user: "ada".into(), password: "pw".into() };
        let message = "Subject: hi\r\n\r\n.hidden line\r\nend\r\n";
        // The greeting is read by `session`; this test starts after it.
        session_after_greeting(&mut stream, &login, "ada@example.com", &["j@example.com".into(), "k@example.com".into()], message).unwrap();
        let written = String::from_utf8(stream.written).unwrap();
        let order = ["EHLO ", "AUTH PLAIN AGFkYQBwdw==", "MAIL FROM:<ada@example.com>", "RCPT TO:<j@example.com>", "RCPT TO:<k@example.com>", "DATA", "Subject: hi", "..hidden line", "\r\n.\r\nQUIT"];
        let mut from = 0;
        for needle in order {
            from += written[from..].find(needle).unwrap_or_else(|| panic!("{needle:?} not found in order in {written:?}"));
        }
    }

    #[test]
    fn oauth_and_refusals() {
        let mut stream = server("250 ok\r\n535 5.7.8 bad credentials\r\n");
        let login = Login::OAuth { user: "ada@gmail.com".into(), token: "tok".into() };
        let error = session_after_greeting(&mut stream, &login, "a@x.org", &["b@x.org".into()], "x").unwrap_err();
        assert!(error.contains("bad credentials"));
        assert!(String::from_utf8(stream.written).unwrap().contains("AUTH XOAUTH2 "));
    }

    #[test]
    fn addresses_cannot_smuggle_commands() {
        assert!(clean("a@x.org\r\nRCPT TO:<evil@x.org>").is_err());
        assert!(clean("<a@x.org>").is_err());
        assert_eq!(clean(" a@x.org ").unwrap(), "a@x.org");
    }

    #[test]
    fn dots_are_doubled_at_line_starts() {
        assert_eq!(dot_stuffed("a\n.b\n..c\n"), "a\r\n..b\r\n...c");
    }
}
