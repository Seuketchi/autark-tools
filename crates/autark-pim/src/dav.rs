//! WebDAV, CalDAV and CardDAV (RFC 4918, 4791, 6352): finding an account's calendars and address books,
//! listing what is in them, and creating, replacing and deleting single items by their ETag. Google, iCloud,
//! Fastmail and Nextcloud all speak this; the first sends an OAuth bearer token, the others a password.

use std::collections::HashMap;
use quick_xml::Reader;
use quick_xml::events::Event;

/// How to prove who you are.
#[derive(Clone, Debug)]
pub enum Auth {
    Basic { user: String, password: String },
    Bearer(String),
}

impl Auth {
    fn header(&self) -> String {
        match self {
            Auth::Basic { user, password } => {
                use base64::Engine;
                format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}")))
            }
            Auth::Bearer(token) => format!("Bearer {token}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    Calendar,
    AddressBook,
}

/// A calendar or an address book on the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    /// Its address, absolute.
    pub url: String,
    pub name: String,
    pub kind: CollectionKind,
    /// For a calendar: whether it holds events, to-dos or both (`VEVENT`, `VTODO`).
    pub components: Vec<String>,
    /// A colour such as `#0088ffff`, if the server has one.
    pub color: String,
}

/// An item as the server has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub url: String,
    pub etag: String,
    pub data: String,
}

pub struct Dav {
    auth: Auth,
}

/// One `response` of a multistatus: its address and its properties by local name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub href: String,
    pub props: HashMap<String, String>,
}

impl Dav {
    pub fn new(auth: Auth) -> Dav {
        Dav { auth }
    }

    /// Sends one request, returning the status, the ETag header and the body.
    fn send(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, String, String), String> {
        let authorization = self.auth.header();
        let mut all: Vec<(&str, &str)> = vec![("Authorization", &authorization)];
        all.extend_from_slice(headers);
        let response = crate::http::request(method, url, &all, body.as_bytes())?;
        let etag = response.header("etag").trim_matches('"').trim_start_matches("W/\"").trim_end_matches('"').to_owned();
        Ok((response.status, etag, String::from_utf8_lossy(&response.body).into_owned()))
    }

    fn multistatus(&self, method: &str, url: &str, depth: &str, body: &str) -> Result<Vec<Entry>, String> {
        let (status, _, text) = self.send(method, url, &[("Depth", depth), ("Content-Type", "application/xml; charset=utf-8")], body)?;
        if status != 207 {
            return Err(format!("{url}: the server answered {status}{}", detail(&text)));
        }
        parse_multistatus(&text, url)
    }

    /// The account's calendars and address books, found from `base`: the server's address, which is where the
    /// current user's principal is asked for.
    pub fn discover(&self, base: &str) -> Result<Vec<Collection>, String> {
        let principal = self.multistatus("PROPFIND", base, "0", PRINCIPAL)?;
        let principal_url = principal.iter().find_map(|entry| entry.props.get("current-user-principal")).map(|href| absolute(base, href)).unwrap_or_else(|| base.to_owned());
        let homes = self.multistatus("PROPFIND", &principal_url, "0", HOMES)?;
        let mut found = Vec::new();
        for (key, kind) in [("calendar-home-set", CollectionKind::Calendar), ("addressbook-home-set", CollectionKind::AddressBook)] {
            let Some(home) = homes.iter().find_map(|entry| entry.props.get(key)) else { continue };
            let home = absolute(&principal_url, home);
            for entry in self.multistatus("PROPFIND", &home, "1", LISTING)? {
                let types = entry.props.get("resourcetype").map(String::as_str).unwrap_or_default();
                let is = match kind {
                    CollectionKind::Calendar => types.split(',').any(|name| name == "calendar"),
                    CollectionKind::AddressBook => types.split(',').any(|name| name == "addressbook"),
                };
                if !is {
                    continue;
                }
                let url = absolute(&home, &entry.href);
                let name = entry.props.get("displayname").cloned().filter(|name| !name.is_empty()).unwrap_or_else(|| url.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_owned());
                let components = entry.props.get("supported-calendar-component-set").map(|set| set.split(',').map(str::to_owned).collect()).unwrap_or_default();
                found.push(Collection { url, name, kind, components, color: entry.props.get("calendar-color").cloned().unwrap_or_default() });
            }
        }
        Ok(found)
    }

    /// The events of a calendar that overlap `[from, to)`, given as UTC `YYYYMMDDTHHMMSSZ`, with repeating
    /// events returned once as the series.
    pub fn events(&self, calendar: &str, from: &str, to: &str) -> Result<Vec<Resource>, String> {
        let body = format!(
            r#"<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{from}" end="{to}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
        );
        self.report(calendar, &body, "calendar-data")
    }

    /// Every to-do of a calendar.
    pub fn todos(&self, calendar: &str) -> Result<Vec<Resource>, String> {
        let body = r#"<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"/></c:comp-filter></c:filter></c:calendar-query>"#;
        self.report(calendar, body, "calendar-data")
    }

    /// Every contact of an address book.
    pub fn contacts(&self, book: &str) -> Result<Vec<Resource>, String> {
        let body = r#"<?xml version="1.0"?><c:addressbook-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:prop><d:getetag/><c:address-data/></d:prop></c:addressbook-query>"#;
        self.report(book, body, "address-data")
    }

    fn report(&self, collection: &str, body: &str, data: &str) -> Result<Vec<Resource>, String> {
        Ok(self
            .multistatus("REPORT", collection, "1", body)?
            .into_iter()
            .filter_map(|entry| {
                let text = entry.props.get(data)?.clone();
                Some(Resource { url: absolute(collection, &entry.href), etag: entry.props.get("getetag").map(|etag| etag.trim_matches('"').trim_start_matches("W/\"").to_owned()).unwrap_or_default(), data: text })
            })
            .collect())
    }

    /// Creates or replaces the item at `url`. With `etag`, the server refuses if the item has changed since
    /// (`Err` says so); without, an existing item is refused instead (`If-None-Match: *`), so a new item never
    /// overwrites another. Returns the new ETag, empty if the server doesn't send one.
    pub fn put(&self, url: &str, data: &str, content_type: &str, etag: Option<&str>) -> Result<String, String> {
        let condition = match etag {
            Some(etag) => ("If-Match", format!("\"{etag}\"")),
            None => ("If-None-Match", "*".to_owned()),
        };
        let (status, new_etag, text) = self.send("PUT", url, &[("Content-Type", content_type), (condition.0, &condition.1)], data)?;
        match status {
            200 | 201 | 204 => Ok(new_etag),
            412 => Err("it has changed on the server since it was read".to_owned()),
            _ => Err(format!("{url}: the server answered {status}{}", detail(&text))),
        }
    }

    /// Deletes the item at `url`, if it is still at `etag`.
    pub fn delete(&self, url: &str, etag: &str) -> Result<(), String> {
        let (status, _, text) = self.send("DELETE", url, &[("If-Match", &format!("\"{etag}\""))], "")?;
        match status {
            200 | 202 | 204 | 404 => Ok(()),
            412 => Err("it has changed on the server since it was read".to_owned()),
            _ => Err(format!("{url}: the server answered {status}{}", detail(&text))),
        }
    }
}

const PRINCIPAL: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;
const HOMES: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="urn:ietf:params:xml:ns:carddav"><d:prop><c:calendar-home-set/><a:addressbook-home-set/></d:prop></d:propfind>"#;
const LISTING: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:i="http://apple.com/ns/ical/"><d:prop><d:resourcetype/><d:displayname/><c:supported-calendar-component-set/><i:calendar-color/></d:prop></d:propfind>"#;

/// Why a server said no, from its body, short.
fn detail(body: &str) -> String {
    let text: String = body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
    if text.is_empty() { String::new() } else { format!(": {text}") }
}

/// `href` made absolute against `base`.
pub fn absolute(base: &str, href: &str) -> String {
    if href.starts_with("https://") || href.starts_with("http://") {
        return href.to_owned();
    }
    let Some(scheme_end) = base.find("://") else { return href.to_owned() };
    let host_end = base[scheme_end + 3..].find('/').map_or(base.len(), |at| scheme_end + 3 + at);
    if href.starts_with('/') {
        format!("{}{}", &base[..host_end], href)
    } else {
        let dir = base[..base.rfind('/').map_or(host_end, |at| at.max(host_end))].trim_end_matches('/');
        format!("{dir}/{href}")
    }
}

/// The `response` elements of a multistatus body. Properties are keyed by local name; an `href` inside a
/// property (a principal, a home set) is keyed by that property; `resourcetype` and
/// `supported-calendar-component-set` list their children, comma-separated. Properties with a non-2xx status
/// (not found) are left out.
pub fn parse_multistatus(xml: &str, _base: &str) -> Result<Vec<Entry>, String> {
    let mut reader = Reader::from_str(xml);
    let mut entries = Vec::new();
    let mut entry: Option<Entry> = None;
    let mut stack: Vec<String> = Vec::new();
    // Properties of the current `propstat`, held until its status says whether they count.
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut ok = true;
    // The text of the innermost element so far, which entities such as `&amp;` split into pieces.
    let mut text = String::new();
    loop {
        let event = reader.read_event().map_err(|err| format!("the server's reply isn't XML: {err}"))?;
        if matches!(event, Event::Start(_) | Event::Empty(_) | Event::End(_)) {
            text_in(&stack, text.trim(), &mut entry, &mut pending, &mut ok);
            text.clear();
        }
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let name = local(start.name().as_ref());
                match name.as_str() {
                    "response" => entry = Some(Entry::default()),
                    "propstat" => {
                        pending.clear();
                        ok = true;
                    }
                    _ => match stack.last().map(String::as_str) {
                        Some("resourcetype") => append(&mut pending, "resourcetype", &name),
                        Some("supported-calendar-component-set") if name == "comp" => {
                            if let Some(component) = start.attributes().flatten().find(|attr| attr.key.as_ref() == b"name") {
                                append(&mut pending, "supported-calendar-component-set", &String::from_utf8_lossy(&component.value));
                            }
                        }
                        _ => {}
                    },
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(name);
                }
            }
            Event::End(_) => match stack.pop().as_deref() {
                Some("propstat") => {
                    if ok && let Some(entry) = &mut entry {
                        for (key, value) in pending.drain(..) {
                            entry.props.entry(key).or_insert(value);
                        }
                    }
                    pending.clear();
                }
                Some("response") => entries.extend(entry.take()),
                _ => {}
            },
            Event::Text(ref piece) => text.push_str(&piece.decode().map_err(|err| err.to_string())?),
            Event::CData(ref data) => text.push_str(&String::from_utf8_lossy(data)),
            Event::GeneralRef(ref reference) => {
                let name = reference.decode().map_err(|err| err.to_string())?;
                match name.as_ref() {
                    "amp" => text.push('&'),
                    "lt" => text.push('<'),
                    "gt" => text.push('>'),
                    "quot" => text.push('"'),
                    "apos" => text.push('\''),
                    other => {
                        let number = other.strip_prefix("#x").and_then(|hex| u32::from_str_radix(hex, 16).ok()).or_else(|| other.strip_prefix('#').and_then(|dec| dec.parse().ok()));
                        text.extend(number.and_then(char::from_u32));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(entries)
}

fn local(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    name.rsplit(':').next().unwrap_or_default().to_owned()
}

fn append(pending: &mut Vec<(String, String)>, key: &str, value: &str) {
    match pending.iter_mut().find(|(name, _)| name == key) {
        Some((_, old)) => {
            old.push(',');
            old.push_str(value);
        }
        None => pending.push((key.to_owned(), value.to_owned())),
    }
}

/// Text found under `stack`'s innermost element.
fn text_in(stack: &[String], value: &str, entry: &mut Option<Entry>, pending: &mut Vec<(String, String)>, ok: &mut bool) {
    if value.is_empty() {
        return;
    }
    let Some(top) = stack.last() else { return };
    let parent = stack.len().checked_sub(2).map(|at| stack[at].as_str()).unwrap_or_default();
    match (top.as_str(), parent) {
        ("href", "response") => {
            if let Some(entry) = entry {
                entry.href = value.to_owned();
            }
        }
        ("status", "propstat") => *ok = value.contains(" 2"),
        // An href inside a property is that property's value.
        ("href", property) => pending.push((property.to_owned(), value.to_owned())),
        (property, "prop") => pending.push((property.to_owned(), value.to_owned())),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principals_and_homes() {
        let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response><d:href>/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/principals/ada/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        let entries = parse_multistatus(xml, "https://x.example/").unwrap();
        assert_eq!(entries[0].href, "/");
        assert_eq!(entries[0].props["current-user-principal"], "/principals/ada/");
    }

    #[test]
    fn collections_with_types_and_missing_properties() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:i="http://apple.com/ns/ical/">
          <d:response><d:href>/cal/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Work &amp; home</d:displayname>
            <c:supported-calendar-component-set><c:comp name="VEVENT"/><c:comp name="VTODO"/></c:supported-calendar-component-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
            <d:propstat><d:prop><i:calendar-color/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response></d:multistatus>"#;
        let entries = parse_multistatus(xml, "https://x.example/").unwrap();
        let props = &entries[0].props;
        assert_eq!(props["resourcetype"], "collection,calendar");
        assert_eq!(props["displayname"], "Work & home");
        assert_eq!(props["supported-calendar-component-set"], "VEVENT,VTODO");
        assert!(!props.contains_key("calendar-color"));
    }

    #[test]
    fn report_data_keeps_its_lines() {
        let xml = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/a.ics</d:href><d:propstat><d:prop><d:getetag>\"abc\"</d:getetag><c:calendar-data>BEGIN:VCALENDAR\nEND:VCALENDAR\n</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
        let entries = parse_multistatus(xml, "https://x.example/").unwrap();
        assert_eq!(entries[0].props["getetag"], "\"abc\"");
        assert!(entries[0].props["calendar-data"].starts_with("BEGIN:VCALENDAR"));
    }

    #[test]
    fn addresses_are_made_absolute() {
        assert_eq!(absolute("https://x.example/a/b/", "/cal/"), "https://x.example/cal/");
        assert_eq!(absolute("https://x.example/a/b/", "c/"), "https://x.example/a/b/c/");
        assert_eq!(absolute("https://x.example", "/cal/"), "https://x.example/cal/");
        assert_eq!(absolute("https://x.example/a", "https://y.example/z"), "https://y.example/z");
    }

    /// A server on 127.0.0.1 that answers each request from `routes` (method, path prefix, status, extra
    /// headers, body) and remembers what it was asked.
    fn serve(routes: Vec<(&'static str, &'static str, u16, &'static str, String)>) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut data = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    data.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&data).into_owned();
                    let Some(end) = text.find("\r\n\r\n") else {
                        if read == 0 { break } else { continue }
                    };
                    let length = text.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|n| n.trim().parse::<usize>().ok())).unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&data).into_owned();
                let mut first = request.lines().next().unwrap_or_default().split_whitespace();
                let (method, path) = (first.next().unwrap_or_default().to_owned(), first.next().unwrap_or_default().to_owned());
                log.lock().unwrap().push(request.clone());
                let route = routes.iter().find(|(m, p, ..)| *m == method && path.starts_with(p));
                let (status, headers, body) = route.map_or((404, "", String::new()), |(_, _, status, headers, body)| (*status, *headers, body.clone()));
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len());
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (port, seen)
    }

    fn multistatus(body: &str) -> String {
        format!("<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">{body}</d:multistatus>")
    }

    #[test]
    fn a_whole_conversation_with_a_server() {
        let principal = multistatus("<d:response><d:href>/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/p/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        let homes = multistatus("<d:response><d:href>/p/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/cal/</d:href></c:calendar-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        let listing = multistatus("<d:response><d:href>/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/cal/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Work</d:displayname><c:supported-calendar-component-set><c:comp name=\"VEVENT\"/></c:supported-calendar-component-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        let events = multistatus("<d:response><d:href>/cal/work/a.ics</d:href><d:propstat><d:prop><d:getetag>\"e1\"</d:getetag><c:calendar-data>BEGIN:VCALENDAR\nEND:VCALENDAR\n</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        let (port, seen) = serve(vec![
            ("PROPFIND", "/cal/", 207, "", listing),
            ("PROPFIND", "/p/", 207, "", homes),
            ("PROPFIND", "/", 207, "", principal),
            ("REPORT", "/cal/work/", 207, "", events),
            ("PUT", "/cal/work/new.ics", 201, "ETag: \"e2\"\r\n", String::new()),
            ("PUT", "/cal/work/old.ics", 412, "", String::new()),
            ("DELETE", "/cal/work/a.ics", 204, "", String::new()),
        ]);
        let base = format!("http://127.0.0.1:{port}/");
        let dav = Dav::new(Auth::Basic { user: "ada".into(), password: "pw".into() });
        let found = dav.discover(&base).unwrap();
        assert_eq!(found, vec![Collection { url: format!("http://127.0.0.1:{port}/cal/work/"), name: "Work".into(), kind: CollectionKind::Calendar, components: vec!["VEVENT".into()], color: String::new() }]);
        let resources = dav.events(&found[0].url, "20261001T000000Z", "20261101T000000Z").unwrap();
        assert_eq!((resources[0].etag.as_str(), resources[0].url.as_str()), ("e1", format!("http://127.0.0.1:{port}/cal/work/a.ics").as_str()));
        assert_eq!(dav.put(&format!("{base}cal/work/new.ics"), "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", "text/calendar", None).unwrap(), "e2");
        assert!(dav.put(&format!("{base}cal/work/old.ics"), "x", "text/calendar", Some("stale")).unwrap_err().contains("changed on the server"));
        dav.delete(&format!("{base}cal/work/a.ics"), "e1").unwrap();
        let seen = seen.lock().unwrap().join("\n---\n").to_ascii_lowercase();
        assert!(seen.contains("authorization: basic ywrhonb3"));
        assert!(seen.contains("if-none-match: *") && seen.contains("if-match: \"stale\"") && seen.contains("if-match: \"e1\""));
        assert!(seen.contains("time-range start=\"20261001t000000z\""));
        assert!(Dav::new(Auth::Bearer("t".into())).discover("http://example.com/").unwrap_err().contains("only https"));
    }

    #[test]
    fn basic_and_bearer_headers() {
        assert_eq!(Auth::Basic { user: "ada".into(), password: "pw".into() }.header(), "Basic YWRhOnB3");
        assert_eq!(Auth::Bearer("t".into()).header(), "Bearer t");
    }
}
