//! Pages a task fetches: https only, named on the task's card, fetched with a plain GET (no cookies, no
//! login), and kept as text, capped like a file.

use std::time::Duration;

use serde_json::{Value, json};

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

const SEARCH_URL: &str = "https://api.firecrawl.dev/v2/search";
/// How many results a search keeps.
const RESULTS: usize = 5;
/// What a lookup for a conversation keeps: a small model on this laptop reads every character slowly, and
/// mixes up labels in a long table, so it gets the best few results, each cut short.
const LOOKUP_RESULTS: usize = 3;
const LOOKUP_WIDTH: usize = 400;

/// Firecrawl's API key: `FIRECRAWL_API_KEY`, else the keyring's (`application = autark-ai`, `key = firecrawl`).
/// Never logged.
pub async fn key(connection: &zbus::Connection) -> Result<String, String> {
    if let Some(key) = std::env::var("FIRECRAWL_API_KEY").ok().filter(|key| !key.is_empty()) {
        return Ok(key);
    }
    crate::jev::stored(connection, "firecrawl").await.map_err(|err| format!("Firecrawl's key: {err}"))
}

/// The best results for `query`, one after another as title, address and snippet. Only search results come
/// back, not whole pages, and no more than a page's worth.
pub fn search(key: &str, query: &str) -> Result<String, String> {
    ask_firecrawl(key, query, RESULTS, usize::MAX)
}

/// `search`, short enough for the local model to read in a conversation.
pub fn lookup(key: &str, query: &str) -> Result<String, String> {
    ask_firecrawl(key, query, LOOKUP_RESULTS, LOOKUP_WIDTH)
}

fn ask_firecrawl(key: &str, query: &str, limit: usize, width: usize) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).https_only(true).http_status_as_error(false).build().into();
    let body = json!({ "query": query, "limit": limit, "sources": ["web"], "timeout": 25000 });
    let response = agent
        .post(SEARCH_URL)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|err| format!("Firecrawl: {err}"))?;
    let status = response.status();
    let text = response.into_body().read_to_string().map_err(|err| format!("Firecrawl's reply: {err}"))?;
    if !status.is_success() {
        let detail: String = text.chars().take(200).collect();
        return Err(format!("Firecrawl: {status} {detail}"));
    }
    let reply: Value = serde_json::from_str(&text).map_err(|err| format!("Firecrawl's reply: {err}"))?;
    Ok(cut(&results(&reply, limit, width)))
}

/// The results in a reply, which groups them by source (`data.web`) or lists them flat (`data`).
/// `width` cuts each snippet to that many characters, its white space (a table's rows, say) run together.
fn results(reply: &Value, limit: usize, width: usize) -> String {
    let data = &reply["data"];
    let items = data["web"].as_array().or_else(|| data.as_array()).map(Vec::as_slice).unwrap_or_default();
    let text: Vec<String> = items
        .iter()
        .take(limit)
        .filter_map(|item| {
            let url = item["url"].as_str().filter(|url| allowed(url).is_ok())?;
            let title = item["title"].as_str().unwrap_or_default().trim();
            let snippet = item["description"].as_str().unwrap_or_default().trim();
            let snippet = if width == usize::MAX { snippet.to_owned() } else { short(snippet, width) };
            Some(format!("{title}\n{url}\n{snippet}"))
        })
        .collect();
    if text.is_empty() { "No results.".to_owned() } else { text.join("\n\n") }
}

/// The top `count` results of a `lookup` as the sources wrote them, for an answer that needs no model; `None`
/// when there is nothing to show. A small model reading a version table mixed up its labels, so a look-up shows the
/// source's own words under its site's name. The panel shows plain text in a narrow column, so there are no
/// addresses to wrap, and the snippet is tidied: no markdown, no bracketed tags, no search-engine ellipses.
pub fn quoted(found: &str, count: usize) -> Option<String> {
    let items: Vec<String> = found
        .split("\n\n")
        .filter_map(|block| {
            let mut lines = block.lines();
            let (_title, url) = (lines.next()?, lines.next()?.trim());
            let host = url.strip_prefix("https://")?.split('/').next()?.trim_start_matches("www.");
            let snippet = tidy(&lines.collect::<Vec<_>>().join(" "));
            (!snippet.is_empty()).then(|| format!("{host}\n{snippet}"))
        })
        .take(count)
        .collect();
    if items.is_empty() { None } else { Some(items.join("\n\n")) }
}

/// A search snippet as a sentence or two: links reduced to their text, `[tags]` and `[12]` dropped, ellipses
/// as separators, and no more than about 240 characters, ending at a word.
fn tidy(snippet: &str) -> String {
    let mut out = String::new();
    let mut rest = snippet;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            rest = "";
            break;
        };
        let inner = &after[..close];
        rest = &after[close + 1..];
        // [text](address) keeps its text; [tarball] and the citation [[12]](address) go.
        if inner.starts_with('[') && rest.starts_with("](") {
            rest = rest.find(')').map_or("", |end| &rest[end + 1..]);
        } else if rest.starts_with('(') {
            if let Some(end) = rest.find(')') {
                if !inner.is_empty() && !inner.starts_with('[') && !inner.chars().all(|c| c.is_ascii_digit()) {
                    out.push_str(inner);
                }
                rest = &rest[end + 1..];
            }
        }
    }
    out.push_str(rest);
    let text = out.replace('…', " · ").replace("...", " · ").replace(['|', '#', '*'], " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_matches(|c: char| c == '·' || c.is_whitespace()).to_owned();
    if text.chars().count() <= 240 {
        return text;
    }
    let kept: String = text.chars().take(240).collect();
    let at = kept.rfind(' ').unwrap_or(kept.len());
    format!("{} …", kept[..at].trim_end_matches(|c: char| c == '·' || c == ',' || c.is_whitespace()))
}

fn short(text: &str, width: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= width {
        return text;
    }
    let kept: String = text.chars().take(width).collect();
    format!("{}…", kept.trim_end())
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
    fn results_come_grouped_or_flat() {
        let item = json!({ "url": "https://a.org/x", "title": " A ", "description": "About a" });
        let bad = json!({ "url": "http://plain.org", "title": "B" });
        let grouped = json!({ "success": true, "data": { "web": [item, bad] } });
        assert_eq!(results(&grouped, RESULTS, usize::MAX), "A\nhttps://a.org/x\nAbout a");
        assert_eq!(results(&json!({ "data": [item] }), RESULTS, usize::MAX), "A\nhttps://a.org/x\nAbout a");
        assert_eq!(results(&json!({}), RESULTS, usize::MAX), "No results.");
    }

    #[test]
    fn a_lookup_keeps_the_best_few_and_cuts_them_short() {
        let long = format!("stable: 7.2.11\n\n| a | b |\n{}", "x".repeat(900));
        let items: Vec<Value> = (1..=5).map(|n| json!({ "title": format!("R{n}"), "url": format!("https://r{n}.org/"), "description": long })).collect();
        let found = results(&json!({ "data": { "web": items } }), LOOKUP_RESULTS, LOOKUP_WIDTH);
        assert_eq!(found.matches("https://").count(), 3);
        assert!(found.contains("stable: 7.2.11 | a | b | xxx"));
        assert!(found.len() < 3 * (LOOKUP_WIDTH + 40));
        assert!(found.contains('…'));
    }

    #[test]
    fn a_lookup_is_quoted_by_site_without_addresses_or_noise() {
        let found = "The Linux Kernel Archives\nhttps://www.kernel.org/\nLatest Release 7.3 mainline: 7.3 2026-10-18 … stable: 7.2.11 2026-10-15 [tarball] [[75]](https://x.org/n) see [the notes](https://x.org/y)\n\nWikipedia\nhttps://en.wikipedia.org/x\nReleases\n\nThird\nhttp://c.org/\nmore";
        let quoted = quoted(found, 2).unwrap();
        assert_eq!(quoted, "kernel.org\nLatest Release 7.3 mainline: 7.3 2026-10-18 · stable: 7.2.11 2026-10-15 see the notes\n\nen.wikipedia.org\nReleases");
        assert!(!quoted.contains("http") && !quoted.contains('['));
        assert_eq!(super::quoted("No results.", 2), None);
        assert_eq!(super::quoted("", 2), None);
    }

    #[test]
    fn a_long_snippet_ends_at_a_word() {
        let long = "word ".repeat(100);
        let tidy = tidy(&long);
        assert!(tidy.chars().count() <= 245 && tidy.ends_with("word …"), "{tidy}");
    }

    #[test]
    fn markup_becomes_text() {
        let html = "<html><head><style>p{}</style><script>var x = '<p>';</script></head><body><h1>Advisories</h1><p>GLib &amp; libxml2:<br>update now</p></body></html>";
        assert_eq!(text_of(html), "Advisories\nGLib & libxml2:\nupdate now");
    }
}
