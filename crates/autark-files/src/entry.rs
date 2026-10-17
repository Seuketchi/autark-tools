//! What a folder holds: one `Entry` per file, with what the views and the info pane show.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use autark_common::files::Trashed;
use gtk::{gio, glib};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub content_type: String,
    /// Set for items shown in the Trash.
    pub trashed: Option<Trashed>,
}

impl Entry {
    pub fn read(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().into_owned();
        // A link shows as what it points at; a broken one as a file.
        let metadata = path.metadata().or_else(|_| path.symlink_metadata()).ok()?;
        let is_dir = metadata.is_dir();
        let content_type = if is_dir { "inode/directory".to_owned() } else { gio::content_type_guess(Some(&name), None).0.to_string() };
        Some(Self { path: path.to_path_buf(), name, is_dir, size: metadata.len(), modified: metadata.modified().ok(), content_type, trashed: None })
    }

    pub fn trashed(item: &Trashed) -> Option<Self> {
        let mut entry = Self::read(&item.path)?;
        if let Some(name) = item.original.file_name() {
            entry.name = name.to_string_lossy().into_owned();
        }
        entry.trashed = Some(item.clone());
        Some(entry)
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    pub fn is_text(&self) -> bool {
        gio::content_type_is_a(&self.content_type, "text/plain")
    }

    /// "Folder", "Markdown document".
    pub fn kind(&self) -> String {
        if self.is_dir {
            return "Folder".to_owned();
        }
        let description = gio::content_type_get_description(&self.content_type).to_string();
        let mut chars = description.chars();
        chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
    }

    /// The badge on a document's icon: its extension, up to four letters.
    pub fn badge(&self) -> String {
        match self.name.rfind('.') {
            Some(dot) if dot > 0 => self.name[dot + 1..].chars().take(4).collect::<String>().to_uppercase(),
            _ => String::new(),
        }
    }

    /// How many items a folder holds, hidden ones left out.
    pub fn children(&self) -> Option<usize> {
        let entries = std::fs::read_dir(&self.path).ok()?;
        Some(entries.flatten().filter(|entry| !entry.file_name().to_string_lossy().starts_with('.')).count())
    }
}

/// A folder's entries, folders first, names in natural order ("2" before "10").
pub fn list(dir: &Path, hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)?.flatten().filter_map(|entry| Entry::read(&entry.path())).filter(|entry| hidden || !entry.is_hidden()).collect();
    sort(&mut entries, Sort::Name, false);
    Ok(entries)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sort {
    Name,
    Size,
    Modified,
    Kind,
}

/// Folders always come first; `descending` flips the order within each group.
pub fn sort(entries: &mut [Entry], by: Sort, descending: bool) {
    entries.sort_by(|a, b| {
        let order = compare(a, b, by);
        b.is_dir.cmp(&a.is_dir).then(if descending { order.reverse() } else { order })
    });
}

/// The order within folders or within files.
pub fn compare(a: &Entry, b: &Entry, by: Sort) -> Ordering {
    let by_name = || natural(&a.name, &b.name);
    match by {
        Sort::Name => by_name(),
        Sort::Size => a.size.cmp(&b.size).then_with(by_name),
        Sort::Modified => a.modified.cmp(&b.modified).then_with(by_name),
        Sort::Kind => a.content_type.cmp(&b.content_type).then_with(by_name),
    }
}

/// Compares names ignoring case, with runs of digits compared as numbers.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    // Case only decides between names that are otherwise the same.
    let mut by_case = Ordering::Equal;
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return by_case,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        chars.next();
                    }
                    digits
                };
                let (x, y) = (take(&mut a), take(&mut b));
                let (tx, ty) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty)).then_with(|| x.len().cmp(&y.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                by_case = by_case.then(x.cmp(&y));
                a.next();
                b.next();
            }
        }
    }
}

/// "4 KB", "1.2 MB", in powers of 1000 as the rest of the desktop shows sizes.
pub fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => format!("{} KB", bytes / 1_000),
        1_000_000..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

/// "Today 14:32", "Yesterday 09:10", "16 Oct 2026".
pub fn date(time: Option<SystemTime>) -> String {
    let Some(seconds) = time.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok()) else {
        return String::new();
    };
    let (Ok(when), Ok(now)) = (glib::DateTime::from_unix_local(seconds.as_secs() as i64), glib::DateTime::now_local()) else {
        return String::new();
    };
    let same_day = |a: &glib::DateTime, b: &glib::DateTime| a.year() == b.year() && a.day_of_year() == b.day_of_year();
    let format = if same_day(&when, &now) {
        "Today %H:%M"
    } else if now.add_days(-1).is_ok_and(|yesterday| same_day(&when, &yesterday)) {
        "Yesterday %H:%M"
    } else {
        "%-d %b %Y"
    };
    when.format(format).map(String::from).unwrap_or_default()
}

/// A path with the home folder shown as `~`.
pub fn tilde(path: &Path) -> String {
    let home = glib::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut names = vec!["file10", "File2", "file1", "a", "B", "file02"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, vec!["a", "B", "file1", "File2", "file02", "file10"]);
    }

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(4_096), "4 KB");
        assert_eq!(size(1_234_567), "1.2 MB");
    }
}
