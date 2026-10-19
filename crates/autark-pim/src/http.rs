//! A small HTTP/1.1 client for WebDAV. The usual HTTP libraries refuse `PROPFIND` and `REPORT`, so this speaks
//! the protocol itself over `tls`: one request per connection (`Connection: close`), bodies by length, by chunks
//! or until the server closes, and redirects followed with the same method and body.

use std::io::{Read, Write};

use crate::tls;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()).unwrap_or_default()
    }
}

/// The most read for one answer.
const LIMIT: usize = 32 * 1024 * 1024;
const REDIRECTS: usize = 5;

struct Url {
    secure: bool,
    host: String,
    port: u16,
    target: String,
}

fn parse(url: &str) -> Result<Url, String> {
    let (secure, rest) = match url.split_once("://") {
        Some(("https", rest)) => (true, rest),
        Some(("http", rest)) => (false, rest),
        _ => return Err(format!("{url}: not an http or https address")),
    };
    let (authority, target) = rest.find('/').map_or((rest, "/".to_owned()), |at| (&rest[..at], rest[at..].to_owned()));
    if authority.contains('@') || authority.is_empty() {
        return Err(format!("{url}: that isn't an address to connect to"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_owned(), port.parse().map_err(|_| format!("{url}: bad port"))?),
        None => (authority.to_owned(), if secure { 443 } else { 80 }),
    };
    Ok(Url { secure, host, port, target })
}

/// Sends the request and returns the final answer.
pub fn request(method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, String> {
    let mut url = url.to_owned();
    for _ in 0..=REDIRECTS {
        let response = once(method, &url, headers, body)?;
        if matches!(response.status, 301 | 302 | 307 | 308) && !response.header("location").is_empty() {
            url = crate::dav::absolute(&url, response.header("location"));
            continue;
        }
        return Ok(response);
    }
    Err(format!("{url}: too many redirects"))
}

fn once(method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, String> {
    let target = parse(url)?;
    let mut head = format!("{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: autark-pim\r\nContent-Length: {}\r\n", target.target, target.host, body.len());
    for (name, value) in headers {
        if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            return Err("a header has a line break in it".to_owned());
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut raw = Vec::new();
    let mut talk = |stream: &mut dyn ReadWrite| -> Result<(), String> {
        stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body)).and_then(|()| stream.flush()).map_err(|err| format!("{url}: {err}"))?;
        let mut limited = stream.take_reader(LIMIT as u64);
        match limited.read_to_end(&mut raw) {
            Ok(_) => Ok(()),
            // A server that closes without TLS's goodbye has still sent its answer.
            Err(err) if !raw.is_empty() && err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(()),
            Err(err) => Err(format!("{url}: {err}")),
        }
    };
    if target.secure {
        let mut stream = tls::connect(&target.host, target.port)?;
        talk(&mut stream)?;
    } else {
        // Plain HTTP is only for a server on this machine.
        if !matches!(target.host.as_str(), "localhost" | "127.0.0.1" | "::1") {
            return Err(format!("{url}: only https servers are used"));
        }
        let mut stream = tls::tcp(&target.host, target.port)?;
        talk(&mut stream)?;
    }
    read_response(&raw, url)
}

/// A stream that can be written, and read through a size limit.
trait ReadWrite: Write {
    fn take_reader(&mut self, limit: u64) -> Box<dyn Read + '_>;
}

impl<T: Read + Write> ReadWrite for T {
    fn take_reader(&mut self, limit: u64) -> Box<dyn Read + '_> {
        Box::new(Read::take(self, limit))
    }
}

fn read_response(raw: &[u8], url: &str) -> Result<Response, String> {
    let end = raw.windows(4).position(|window| window == b"\r\n\r\n").ok_or_else(|| format!("{url}: the server's answer was cut short"))?;
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let mut lines = head.lines();
    let status: u16 = lines.next().and_then(|line| line.split_whitespace().nth(1)).and_then(|code| code.parse().ok()).ok_or_else(|| format!("{url}: the server's answer isn't HTTP"))?;
    let headers: Vec<(String, String)> = lines.filter_map(|line| line.split_once(':')).map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned())).collect();
    let rest = &raw[end + 4..];
    let find = |name: &str| headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    let body = if find("transfer-encoding").is_some_and(|value| value.to_ascii_lowercase().contains("chunked")) {
        dechunk(rest).ok_or_else(|| format!("{url}: the server's answer was cut short"))?
    } else if let Some(length) = find("content-length").and_then(|value| value.parse::<usize>().ok()) {
        rest.get(..length).ok_or_else(|| format!("{url}: the server's answer was cut short"))?.to_vec()
    } else {
        rest.to_vec()
    };
    Ok(Response { status, headers, body })
}

fn dechunk(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|pair| pair == b"\r\n")?;
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(data.get(..size)?);
        data = data.get(size + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_by_length_by_chunks_and_until_close() {
        let by_length = read_response(b"HTTP/1.1 207 Multi\r\nETag: \"x\"\r\nContent-Length: 5\r\n\r\nhelloEXTRA", "u").unwrap();
        assert_eq!((by_length.status, by_length.body.as_slice(), by_length.header("etag")), (207, b"hello".as_slice(), "\"x\""));
        let chunked = read_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n", "u").unwrap();
        assert_eq!(chunked.body, b"hello world");
        assert_eq!(read_response(b"HTTP/1.0 200 OK\r\n\r\nrest", "u").unwrap().body, b"rest");
        assert!(read_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort", "u").is_err());
        assert!(read_response(b"garbage", "u").is_err());
    }

    #[test]
    fn addresses() {
        let url = parse("https://cal.example.com:8443/dav/a?x=1").unwrap();
        assert_eq!((url.secure, url.host.as_str(), url.port, url.target.as_str()), (true, "cal.example.com", 8443, "/dav/a?x=1"));
        assert_eq!(parse("http://localhost").unwrap().target, "/");
        assert!(parse("https://me:pw@host/").is_err() && parse("ftp://x/").is_err());
        assert!(once("GET", "http://example.com/", &[], b"").unwrap_err().contains("only https"));
    }
}
