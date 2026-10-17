//! Pages a task fetches: https only, named on the task's card, fetched with a plain GET (no cookies, no
//! login), and kept as text, capped like a file.

use std::time::Duration;

/// The most kept from one page, after its markup is gone.
const KEPT: usize = 8 * 1024;
/// The most read from the network for one page.
const READ: u64 = 2 * 1024 * 1024;

/// Whether a task may fetch `url`: https, a host, and no name or password in it.
pub fn allowed(url: &str) -> Result<(), String> {
    let rest = url.trim().strip_prefix("https://").ok_or_else(|| format!("{url}: only https pages can be fetched"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() || host.contains('@') || rest.contains(char::is_whitespace) {
        return Err(format!("{url}: that isn't a page Kith can fetch"));
    }
    Ok(())
}

/// The `https://` addresses written in `text`, in order, each once.
pub fn named(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for word in text.split_whitespace() {
        let word = word.trim_matches(|c: char| "\"'`“”‘’()[]{}<>,;!".contains(c)).trim_end_matches(['.', ':', '?']);
        if word.starts_with("https://") && allowed(word).is_ok() && !found.iter().any(|seen| seen == word) {
            found.push(word.to_owned());
        }
    }
    found
}

/// The page's text, its markup taken out.
pub fn fetch(url: &str) -> Result<String, String> {
    allowed(url)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).https_only(true).http_status_as_error(false).build().into();
    let response = agent.get(url).call().map_err(|err| format!("{url}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("{url}: {}", response.status()));
    }
    let html = response.headers().get("content-type").and_then(|value| value.to_str().ok()).is_none_or(|kind| kind.contains("html"));
    let body = response.into_body().with_config().limit(READ).read_to_string().map_err(|err| format!("{url}: {err}"))?;
    let text = if html { text_of(&body) } else { body };
    Ok(cut(&text))
}

fn cut(text: &str) -> String {
    if text.len() <= KEPT {
        return text.to_owned();
    }
    let mut end = KEPT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n(the page goes on)", &text[..end])
}

/// HTML as plain text: scripts, styles and tags dropped, block ends as new lines, common entities decoded.
fn text_of(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tag_end = rest[start..].find('>').map_or(rest.len(), |end| start + end + 1);
        let tag = rest[start..tag_end].to_lowercase();
        rest = &rest[tag_end..];
        let name: String = tag.trim_start_matches(['<', '/']).chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if (name == "script" || name == "style") && !tag.starts_with("</") {
            let close = format!("</{name}");
            rest = find_ignoring_case(rest, &close).map_or("", |at| &rest[at..]);
            continue;
        }
        if ["p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "table", "section", "article"].contains(&name.as_str()) {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    out.push_str(rest);
    let decoded = out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    let lines: Vec<String> = decoded.lines().map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|line| !line.is_empty()).collect();
    lines.join("\n")
}

/// Where ASCII `needle` is in `haystack`, whatever the case of either.
fn find_ignoring_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack.as_bytes().windows(needle.len()).position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_https_pages() {
        assert!(allowed("https://www.linuxfromscratch.org/blfs/advisories/").is_ok());
        assert!(allowed("http://example.com").is_err());
        assert!(allowed("https://me:secret@example.com").is_err());
        assert!(allowed("file:///etc/passwd").is_err());
        assert_eq!(named("check https://a.org/x, then http://b.org and https://a.org/x."), vec!["https://a.org/x"]);
    }

    #[test]
    fn markup_becomes_text() {
        let html = "<html><head><style>p{}</style><script>var x = '<p>';</script></head><body><h1>Advisories</h1><p>GLib &amp; libxml2:<br>update now</p></body></html>";
        assert_eq!(text_of(html), "Advisories\nGLib & libxml2:\nupdate now");
    }
}
