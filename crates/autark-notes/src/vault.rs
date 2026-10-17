//! A vault is any folder of Markdown files. Folders are folders on disk, a note's title is its
//! file name, and links use Obsidian's `[[note name]]`, so an Obsidian vault opens unchanged.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Note {
    /// Relative to the vault, with `/` separators and the `.md` extension.
    pub path: String,
    pub title: String,
    /// The folder it's in, relative to the vault; empty for the top level.
    pub folder: String,
    pub modified: SystemTime,
    pub text: String,
    /// Link targets as written, without `#heading` or `|alias`.
    pub links: Vec<String>,
    /// Without the `#`.
    pub tags: Vec<String>,
    pub preview: String,
}

pub struct Vault {
    pub root: PathBuf,
    pub notes: Vec<Note>,
    /// Every folder, relative to the vault, sorted; the top level isn't listed.
    pub folders: Vec<String>,
}

impl Vault {
    pub fn scan(root: &Path) -> Self {
        let mut vault = Self { root: root.to_owned(), notes: Vec::new(), folders: Vec::new() };
        vault.walk(root, "");
        vault.folders.sort_by_key(|folder| folder.to_lowercase());
        vault
    }

    fn walk(&mut self, dir: &Path, rel: &str) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Hidden folders hold app state: ours, Obsidian's, git's.
            if name.starts_with('.') {
                continue;
            }
            let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                self.folders.push(child.clone());
                self.walk(&entry.path(), &child);
            } else if let Some(title) = name.strip_suffix(".md") {
                let Ok(text) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };
                let modified = entry.metadata().and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                self.notes.push(Note::new(child, title.to_owned(), rel.to_owned(), modified, text));
            }
        }
    }

    pub fn get(&self, path: &str) -> Option<&Note> {
        self.notes.iter().find(|note| note.path == path)
    }

    pub fn get_mut(&mut self, path: &str) -> Option<&mut Note> {
        self.notes.iter_mut().find(|note| note.path == path)
    }

    pub fn full_path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    /// The note a link points at: a path match first (`folder/Name`), then a title match, both
    /// ignoring case; among equal titles the shallowest wins, as in Obsidian.
    pub fn resolve(&self, target: &str) -> Option<&Note> {
        let target = target.trim().trim_end_matches(".md").to_lowercase();
        if target.is_empty() {
            return None;
        }
        if let Some(note) = self.notes.iter().find(|note| note.path.trim_end_matches(".md").to_lowercase() == target) {
            return Some(note);
        }
        self.notes.iter().filter(|note| note.title.to_lowercase() == target).min_by_key(|note| note.path.matches('/').count())
    }

    /// Notes with a link to `path`, by title.
    pub fn backlinks(&self, path: &str) -> Vec<&Note> {
        let mut found: Vec<&Note> = self
            .notes
            .iter()
            .filter(|note| note.path != path && note.links.iter().any(|link| self.resolve(link).is_some_and(|target| target.path == path)))
            .collect();
        found.sort_by_key(|note| note.title.to_lowercase());
        found
    }

    /// Every tag with how many notes use it, sorted by name.
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut tags: Vec<(String, usize)> = Vec::new();
        for tag in self.notes.iter().flat_map(|note| &note.tags) {
            match tags.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(tag)) {
                Some((_, count)) => *count += 1,
                None => tags.push((tag.clone(), 1)),
            }
        }
        tags.sort_by_key(|(name, _)| name.to_lowercase());
        tags
    }

    /// A file name for a new note in `folder` that isn't taken: "Untitled", "Untitled 2", ….
    pub fn free_path(&self, folder: &str, title: &str) -> String {
        (1..)
            .map(|n| if n == 1 { title.to_owned() } else { format!("{title} {n}") })
            .map(|name| if folder.is_empty() { format!("{name}.md") } else { format!("{folder}/{name}.md") })
            .find(|path| self.get(path).is_none() && !self.full_path(path).exists())
            .expect("an unbounded range always finds a free name")
    }

    /// Text with every link to `old` (a title) pointed at `new`, keeping any `#heading` or
    /// `|alias`. Returns `None` when nothing changed.
    pub fn relink(text: &str, old: &str, new: &str) -> Option<String> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        let mut changed = false;
        while let Some(start) = rest.find("[[") {
            out.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            let Some(end) = rest.find("]]") else {
                break;
            };
            let inner = &rest[..end];
            let split = inner.find(['#', '|']).unwrap_or(inner.len());
            if inner[..split].trim().eq_ignore_ascii_case(old) {
                out.push_str(new);
                out.push_str(&inner[split..]);
                changed = true;
            } else {
                out.push_str(inner);
            }
            rest = &rest[end..];
        }
        out.push_str(rest);
        changed.then_some(out)
    }
}

impl Note {
    pub fn new(path: String, title: String, folder: String, modified: SystemTime, text: String) -> Self {
        let mut note = Self { path, title, folder, modified, text: String::new(), links: Vec::new(), tags: Vec::new(), preview: String::new() };
        note.set_text(text);
        note
    }

    pub fn set_text(&mut self, text: String) {
        self.links = links(&text);
        self.tags = tags(&text);
        self.preview = preview(&text, &self.title);
        self.text = text;
    }

    /// True when every word of `query` is in the title or the text, ignoring case.
    pub fn matches(&self, query: &str) -> bool {
        let title = self.title.to_lowercase();
        let text = self.text.to_lowercase();
        query.split_whitespace().map(str::to_lowercase).all(|word| title.contains(&word) || text.contains(&word))
    }
}

/// YAML front matter, if any, and the body after it.
fn split_front_matter(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---")
    {
        let body = &rest[end + 4..];
        return (&rest[..end], body.strip_prefix('\n').unwrap_or(body));
    }
    ("", text)
}

/// Lines outside fenced code blocks.
fn prose_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut fenced = false;
    text.lines().filter(move |line| {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            return false;
        }
        !fenced
    })
}

/// A line with inline code spans blanked out, so links and tags inside them don't count.
fn without_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for c in line.chars() {
        if c == '`' {
            in_code = !in_code;
        }
        out.push(if in_code || c == '`' { ' ' } else { c });
    }
    out
}

fn links(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in prose_lines(split_front_matter(text).1) {
        let line = without_code(line);
        let mut rest = line.as_str();
        while let Some(start) = rest.find("[[") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find("]]") else {
                break;
            };
            let inner = &rest[..end];
            let target = inner[..inner.find(['#', '|']).unwrap_or(inner.len())].trim();
            if !target.is_empty() && !found.iter().any(|link| link.eq_ignore_ascii_case(target)) {
                found.push(target.to_owned());
            }
            rest = &rest[end + 2..];
        }
    }
    found
}

pub fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

fn tags(text: &str) -> Vec<String> {
    let (front, body) = split_front_matter(text);
    let mut found: Vec<String> = Vec::new();
    let mut add = |tag: &str| {
        let tag = tag.trim().trim_start_matches('#');
        if !tag.is_empty() && tag.chars().all(is_tag_char) && !tag.chars().all(|c| c.is_ascii_digit()) && !found.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            found.push(tag.to_owned());
        }
    };
    // Front matter: `tags: [a, b]`, `tags: a, b`, or a `- a` list under `tags:`.
    let mut in_list = false;
    for line in front.lines() {
        if let Some(value) = line.strip_prefix("tags:") {
            value.trim().trim_start_matches('[').trim_end_matches(']').split([',', ' ']).for_each(&mut add);
            in_list = value.trim().is_empty();
        } else if in_list && let Some(item) = line.trim_start().strip_prefix("- ") {
            add(item);
        } else {
            in_list = false;
        }
    }
    for line in prose_lines(body) {
        let line = without_code(line);
        for (i, c) in line.char_indices() {
            let starts_word = line[..i].chars().next_back().is_none_or(char::is_whitespace);
            if c == '#' && starts_word {
                let tag: String = line[i + 1..].chars().take_while(|&c| is_tag_char(c)).collect();
                add(&tag);
            }
        }
    }
    found
}

/// The first line of text worth showing in the list, with Markdown markers dropped and links
/// shown as their names.
fn preview(text: &str, title: &str) -> String {
    let line = prose_lines(split_front_matter(text).1)
        .map(str::trim)
        .find(|line| !line.is_empty() && line.trim_start_matches('#').trim() != title && *line != "---")
        .unwrap_or("");
    let line = line.trim_start_matches(['#', '>', ' ']);
    let line = line.strip_prefix("- [ ] ").or_else(|| line.strip_prefix("- [x] ")).or_else(|| line.strip_prefix("- ")).unwrap_or(line);
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else {
            break;
        };
        let inner = &rest[..end];
        out.push_str(inner.rsplit('|').next().unwrap_or(inner));
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    let out: String = out.chars().filter(|c| !matches!(c, '*' | '`')).collect();
    out.chars().take(140).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(path: &str, text: &str) -> Note {
        let title = path.rsplit('/').next().unwrap().trim_end_matches(".md");
        let folder = path.rsplit_once('/').map(|(folder, _)| folder).unwrap_or("");
        Note::new(path.to_owned(), title.to_owned(), folder.to_owned(), SystemTime::UNIX_EPOCH, text.to_owned())
    }

    #[test]
    fn links_and_tags() {
        let n = note("Kith.md", "---\ntags: [kith, autark]\n---\nRuns on [[Hermes]] and [[Memory rules|rules]].\nSee [[roadmap#M7]], #weekly #2026 `[[not a link]] #notag`\n```\n[[fenced]] #fenced\n```\n# Heading");
        assert_eq!(n.links, ["Hermes", "Memory rules", "roadmap"]);
        assert_eq!(n.tags, ["kith", "autark", "weekly"]);
        assert_eq!(n.preview, "Runs on Hermes and rules.");
    }

    #[test]
    fn resolve_and_backlinks() {
        let vault = Vault {
            root: PathBuf::new(),
            notes: vec![note("Autark/Kith.md", "[[hermes]]"), note("Hermes.md", "[[Kith]]"), note("Old/Hermes.md", ""), note("Roadmap.md", "[[Autark/Kith]] [[nowhere]]")],
            folders: vec![],
        };
        assert_eq!(vault.resolve("hermes").unwrap().path, "Hermes.md");
        assert_eq!(vault.resolve("Old/Hermes").unwrap().path, "Old/Hermes.md");
        let back: Vec<&str> = vault.backlinks("Autark/Kith.md").iter().map(|n| n.path.as_str()).collect();
        assert_eq!(back, ["Hermes.md", "Roadmap.md"]);
    }

    #[test]
    fn relink_keeps_headings_and_aliases() {
        let text = "[[Kith]] and [[kith#Memory]] and [[Kith|the assistant]] but not [[Kithara]]";
        assert_eq!(Vault::relink(text, "Kith", "Assistant").unwrap(), "[[Assistant]] and [[Assistant#Memory]] and [[Assistant|the assistant]] but not [[Kithara]]");
        assert_eq!(Vault::relink("[[Other]]", "Kith", "X"), None);
    }

    #[test]
    fn search_needs_every_word() {
        let n = note("Build log.md", "Kernel config done");
        assert!(n.matches("kernel BUILD"));
        assert!(!n.matches("kernel wifi"));
    }
}
