//! The tools' own config files, `~/.config/autark/<name>.toml`, edited in place: comments, order and keys
//! Settings doesn't know about all stay. The first change copies `/etc/autark/<name>.toml` across, since the
//! user's file replaces the system's rather than adding to it.

use std::path::PathBuf;

use toml_edit::{DocumentMut, Item, Table, Value};

pub fn path(name: &str) -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("autark").join(format!("{name}.toml"))
}

/// The file Settings would edit: the user's, else the system's, else nothing.
pub fn read(name: &str) -> DocumentMut {
    let user = std::fs::read_to_string(path(name));
    let text = user.or_else(|_| std::fs::read_to_string(format!("/etc/autark/{name}.toml"))).unwrap_or_default();
    text.parse().unwrap_or_default()
}

fn find<'a>(document: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    let mut item = document.as_item();
    for part in key.split('.') {
        item = item.get(part)?;
    }
    Some(item)
}

pub fn get_bool(document: &DocumentMut, key: &str) -> Option<bool> {
    find(document, key)?.as_bool()
}

pub fn get_int(document: &DocumentMut, key: &str) -> Option<i64> {
    find(document, key)?.as_integer()
}

pub fn get_float(document: &DocumentMut, key: &str) -> Option<f64> {
    let item = find(document, key)?;
    item.as_float().or_else(|| item.as_integer().map(|value| value as f64))
}

pub fn get_str(document: &DocumentMut, key: &str) -> Option<String> {
    find(document, key)?.as_str().map(str::to_owned)
}

/// Sets `key` (dotted for tables, `night.from`) to `value`, keeping the rest of the file as it is.
pub fn set_in(document: &mut DocumentMut, key: &str, value: impl Into<Value>) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let Some(last) = parts.pop() else { return };
    let mut table: &mut Table = document.as_table_mut();
    for part in parts {
        if !table.contains_table(part) {
            let mut new = Table::new();
            new.set_implicit(true);
            table.insert(part, Item::Table(new));
        }
        let Some(next) = table.get_mut(part).and_then(Item::as_table_mut) else { return };
        table = next;
    }
    match table.get_mut(last).and_then(Item::as_value_mut) {
        // Keep the comment after the old value.
        Some(old) => {
            let decor = old.decor().clone();
            *old = value.into();
            *old.decor_mut() = decor;
        }
        None => {
            table.insert(last, Item::Value(value.into()));
        }
    }
}

pub fn remove_in(document: &mut DocumentMut, key: &str) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let Some(last) = parts.pop() else { return };
    let mut table: &mut Table = document.as_table_mut();
    for part in parts {
        let Some(next) = table.get_mut(part).and_then(Item::as_table_mut) else { return };
        table = next;
    }
    table.remove(last);
}

pub fn write(name: &str, document: &DocumentMut) -> Result<(), String> {
    let path = path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    autark_common::files::write_atomically(&path, document.to_string().as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

/// Reads, changes and writes one file.
pub fn edit(name: &str, change: impl FnOnce(&mut DocumentMut)) -> Result<(), String> {
    let mut document = read(name);
    change(&mut document);
    write(name, &document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_comments_and_unknown_keys() {
        let mut document: DocumentMut = "# my dock\nsize = 48 # px\nmine = \"kept\"\n".parse().unwrap();
        set_in(&mut document, "size", 56);
        set_in(&mut document, "magnify", false);
        set_in(&mut document, "night.from", "22:00");
        assert_eq!(get_int(&document, "size"), Some(56));
        assert_eq!(get_str(&document, "night.from").as_deref(), Some("22:00"));
        let text = document.to_string();
        assert!(text.starts_with("# my dock\nsize = 56 # px\nmine = \"kept\"\nmagnify = false\n"), "{text}");
        assert!(text.contains("[night]\nfrom = \"22:00\""), "{text}");
        remove_in(&mut document, "magnify");
        assert_eq!(get_bool(&document, "magnify"), None);
    }
}
