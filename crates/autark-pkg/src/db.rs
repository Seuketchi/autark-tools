//! What's installed: `<db>/<name>/info.toml` (version, dependencies, the archive it came from) and
//! `<db>/<name>/files`, one line per path the package owns:
//!
//! ```text
//! f <sha256 or -> /usr/bin/foo     a file, with the hash it was installed with
//! l /usr/lib/libfoo.so             a symlink
//! d /usr/share/foo                 a folder the package created, removed with it when empty
//! ```

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Context, Result, fail};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Installed {
    pub name: String,
    /// `version-release`.
    pub version: String,
    #[serde(default)]
    pub depends: Vec<String>,
    pub mode: String,
    /// Seconds since the epoch.
    pub installed: u64,
    /// The cached archive it was installed from; none for packages imported from the phase scripts' lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    /// Set by a rollback, so `upgrade` leaves the package alone until it's installed again by name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    File(Option<String>),
    Link,
    Dir,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Absolute on the target system.
    pub path: String,
    pub kind: Kind,
}

pub struct Db {
    dir: PathBuf,
    _lock: File,
}

impl Db {
    /// Opens the database, waiting for any other autark-pkg to finish.
    pub fn open(config: &Config) -> Result<Self> {
        std::fs::create_dir_all(&config.db).ctx(&config.db)?;
        let path = config.db.join(".lock");
        let lock = File::create(&path).ctx(&path)?;
        if lock.try_lock().is_err() {
            println!("waiting for another autark-pkg to finish");
            lock.lock().ctx(&path)?;
        }
        Ok(Self { dir: config.db.clone(), _lock: lock })
    }

    pub fn names(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.dir).ctx(&self.dir)? {
            let entry = entry.ctx(&self.dir)?;
            if entry.path().join("info.toml").exists() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn get(&self, name: &str) -> Result<Option<Installed>> {
        let path = self.dir.join(name).join("info.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(toml::from_str(&text).ctx(&path)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => fail(format!("{}: {error}", path.display())),
        }
    }

    pub fn all(&self) -> Result<BTreeMap<String, Installed>> {
        let mut out = BTreeMap::new();
        for name in self.names()? {
            if let Some(info) = self.get(&name)? {
                out.insert(name, info);
            }
        }
        Ok(out)
    }

    pub fn files(&self, name: &str) -> Result<Vec<Item>> {
        let path = self.dir.join(name).join("files");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return fail(format!("{}: {error}", path.display())),
        };
        text.lines().filter(|line| !line.is_empty()).map(|line| parse_item(line).ok_or_else(|| crate::error::Error(format!("{}: bad line {line:?}", path.display())))).collect()
    }

    pub fn save(&self, info: &Installed, items: &[Item]) -> Result<()> {
        let dir = self.dir.join(&info.name);
        std::fs::create_dir_all(&dir).ctx(&dir)?;
        let mut files = String::new();
        for item in items {
            files.push_str(&format_item(item));
            files.push('\n');
        }
        write_atomically(&dir.join("files"), files.as_bytes())?;
        write_atomically(&dir.join("info.toml"), toml::to_string(info).ctx("info.toml")?.as_bytes())
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let dir = self.dir.join(name);
        std::fs::remove_dir_all(&dir).ctx(&dir)
    }

    /// Which package owns each file and symlink. Folders can be shared and aren't listed.
    pub fn owners(&self) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        for name in self.names()? {
            for item in self.files(&name)? {
                if item.kind != Kind::Dir {
                    out.insert(item.path, name.clone());
                }
            }
        }
        Ok(out)
    }

    /// Folders each package created, for not removing one another package still records.
    pub fn dirs(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for name in self.names()? {
            for item in self.files(&name)? {
                if item.kind == Kind::Dir {
                    out.entry(item.path).or_default().push(name.clone());
                }
            }
        }
        Ok(out)
    }
}

fn parse_item(line: &str) -> Option<Item> {
    let (tag, rest) = line.split_once(' ')?;
    match tag {
        "f" => {
            let (hash, path) = rest.split_once(' ')?;
            Some(Item { path: path.to_owned(), kind: Kind::File((hash != "-").then(|| hash.to_owned())) })
        }
        "l" => Some(Item { path: rest.to_owned(), kind: Kind::Link }),
        "d" => Some(Item { path: rest.to_owned(), kind: Kind::Dir }),
        _ => None,
    }
}

fn format_item(item: &Item) -> String {
    match &item.kind {
        Kind::File(hash) => format!("f {} {}", hash.as_deref().unwrap_or("-"), item.path),
        Kind::Link => format!("l {}", item.path),
        Kind::Dir => format!("d {}", item.path),
    }
}

/// Writes through a temporary file and a rename, so a crash leaves the old contents or the new, never half.
pub fn write_atomically(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_file_name(format!(".{}.{}", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let mut file = File::create(&temporary).ctx(&temporary)?;
    file.write_all(contents).ctx(&temporary)?;
    file.sync_all().ctx(&temporary)?;
    std::fs::rename(&temporary, path).ctx(path)
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_round_trip() {
        for line in ["f 0a1b /usr/bin/a b", "f - /etc/x", "l /usr/lib/libz.so", "d /usr/share/z"] {
            assert_eq!(format_item(&parse_item(line).unwrap()), line);
        }
        assert_eq!(parse_item("x /y"), None);
    }
}
