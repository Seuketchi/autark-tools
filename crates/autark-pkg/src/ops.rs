//! Installing from a verified archive and removing, with the file lists keeping both clean: an upgrade or
//! rollback removes what the old build had and the new one doesn't. Files are replaced by rename, so a running
//! program keeps its old copy. A changed file in /etc is never overwritten or removed: the package's version goes
//! beside it as `.autark-new`.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use crate::archive::{self, Entry, EntryKind};
use crate::build::is_root;
use crate::config::Config;
use crate::db::{self, Db, Installed, Item, Kind};
use crate::error::{Context, Result, fail};
use crate::fetch;

pub struct Install<'a> {
    pub archive: &'a Path,
    /// False when a tracked build already put the files in place.
    pub extract: bool,
    pub held: bool,
    /// Take files over from the packages that own them.
    pub force: bool,
}

pub fn install(config: &Config, db: &Db, how: &Install) -> Result<()> {
    let info = archive::verify(config, how.archive)?;
    let old = db.files(&info.name)?;
    let old_hashes: HashMap<&str, &Option<String>> = old.iter().filter_map(|item| if let Kind::File(hash) = &item.kind { Some((item.path.as_str(), hash)) } else { None }).collect();
    let owners = db.owners()?;

    let mut entries = Vec::new();
    archive::walk(how.archive, &mut |entry: &Entry, _: &mut dyn Read| {
        entries.push(entry.clone());
        Ok(())
    })?;
    let mut taken: HashMap<&str, Vec<String>> = HashMap::new();
    for entry in entries.iter().filter(|entry| entry.kind != EntryKind::Dir) {
        if let Some(owner) = owners.get(&entry.path).filter(|owner| **owner != info.name) {
            taken.entry(owner.as_str()).or_default().push(entry.path.clone());
        }
    }
    if !taken.is_empty() && how.extract && !how.force {
        let mut lines: Vec<String> = taken.iter().flat_map(|(owner, paths)| paths.iter().map(move |path| format!("  {path} (owned by {owner})"))).collect();
        lines.sort();
        let more = lines.len().saturating_sub(10);
        lines.truncate(10);
        let tail = if more > 0 { format!("\n  … and {more} more") } else { String::new() };
        return fail(format!("{} {} would overwrite files other packages own:\n{}{tail}\n--force takes them over.", info.name, info.version, lines.join("\n")));
    }
    for (owner, paths) in &taken {
        println!("   takes {} path(s) over from {owner}", paths.len());
        disown(db, owner, paths)?;
    }

    let old_dirs: HashSet<&str> = old.iter().filter(|item| item.kind == Kind::Dir).map(|item| item.path.as_str()).collect();
    let mut items = Vec::new();
    if how.extract {
        let root = is_root();
        archive::walk(how.archive, &mut |entry: &Entry, contents: &mut dyn Read| {
            let target = config.path(&entry.path);
            match &entry.kind {
                EntryKind::Dir => {
                    let existed = target.is_dir();
                    if !existed {
                        std::fs::create_dir_all(&target).ctx(&target)?;
                        set_owner(&target, entry, root)?;
                        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(entry.mode)).ctx(&target)?;
                    }
                    if !existed || old_dirs.contains(entry.path.as_str()) {
                        items.push(Item { path: entry.path.clone(), kind: Kind::Dir });
                    }
                }
                EntryKind::File => {
                    let protect = is_config(&entry.path)
                        && target.exists()
                        && match old_hashes.get(entry.path.as_str()) {
                            Some(Some(hash)) => fetch::sha256(&target).ok().as_ref() != Some(hash),
                            Some(None) => true,
                            None => !owners.contains_key(&entry.path),
                        };
                    let destination = if protect { target.with_file_name(format!("{}.autark-new", file_name(&target))) } else { target.clone() };
                    let hash = write_file(&destination, entry, contents, root)?;
                    if protect {
                        println!("   kept your {}; the package's version is {}.autark-new", entry.path, entry.path);
                    }
                    items.push(Item { path: entry.path.clone(), kind: Kind::File(Some(hash)) });
                }
                EntryKind::Link(link) => {
                    if target.is_dir() && !target.is_symlink() {
                        return fail(format!("{}: a folder is in the way of a symlink", target.display()));
                    }
                    let temporary = temporary(&target);
                    let _ = std::fs::remove_file(&temporary);
                    std::os::unix::fs::symlink(link, &temporary).ctx(&temporary)?;
                    if root {
                        std::os::unix::fs::lchown(&temporary, Some(entry.uid), Some(entry.gid)).ctx(&temporary)?;
                    }
                    std::fs::rename(&temporary, &target).ctx(&target)?;
                    items.push(Item { path: entry.path.clone(), kind: Kind::Link });
                }
            }
            Ok(())
        })?;
    } else {
        for entry in &entries {
            let kind = match entry.kind {
                EntryKind::Dir => Kind::Dir,
                EntryKind::Link(_) => Kind::Link,
                EntryKind::File => Kind::File(Some(fetch::sha256(&config.path(&entry.path))?)),
            };
            items.push(Item { path: entry.path.clone(), kind });
        }
    }

    // What the old build had and this one doesn't.
    let now: HashSet<&str> = items.iter().map(|item| item.path.as_str()).collect();
    let leftovers: Vec<Item> = old.iter().filter(|item| !now.contains(item.path.as_str())).cloned().collect();
    remove_items(config, db, &info.name, &leftovers)?;

    let installed = Installed {
        name: info.name.clone(),
        version: info.version.clone(),
        depends: info.depends.clone(),
        mode: info.mode.clone(),
        installed: db::now(),
        archive: Some(info.file_name()),
        held: how.held,
    };
    db.save(&installed, &items)?;
    ldconfig(config, &items);
    println!("   installed {} {} ({} paths)", info.name, info.version, items.len());
    Ok(())
}

pub fn remove(config: &Config, db: &Db, name: &str) -> Result<()> {
    let items = db.files(name)?;
    remove_items(config, db, name, &items)?;
    db.delete(name)?;
    ldconfig(config, &items);
    println!("   removed {name}");
    Ok(())
}

/// Removes `items` that `name` still owns: files and symlinks, then the folders it created once they're empty.
fn remove_items(config: &Config, db: &Db, name: &str, items: &[Item]) -> Result<()> {
    let owners = db.owners()?;
    for item in items.iter().filter(|item| item.kind != Kind::Dir) {
        if owners.get(&item.path).is_some_and(|owner| owner != name) {
            continue;
        }
        let path = config.path(&item.path);
        if let Kind::File(hash) = &item.kind
            && is_config(&item.path)
            && path.exists()
            && (hash.is_none() || fetch::sha256(&path).ok() != *hash)
        {
            println!("   kept {} (changed since it was installed)", item.path);
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return fail(format!("{}: {error}", path.display())),
        }
    }
    let shared = db.dirs()?;
    let mut dirs: Vec<&Item> = items.iter().filter(|item| item.kind == Kind::Dir && !shared.get(&item.path).is_some_and(|names| names.iter().any(|other| other != name))).collect();
    dirs.sort_by_key(|item| std::cmp::Reverse(item.path.matches('/').count()));
    for item in dirs {
        // Fails while anything is left inside, which is what keeps folders others use.
        let _ = std::fs::remove_dir(config.path(&item.path));
    }
    Ok(())
}

/// Drops `paths` from another package's file list, when a newer package takes them over.
fn disown(db: &Db, owner: &str, paths: &[String]) -> Result<()> {
    let Some(info) = db.get(owner)? else {
        return Ok(());
    };
    let gone: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let items: Vec<Item> = db.files(owner)?.into_iter().filter(|item| !gone.contains(item.path.as_str())).collect();
    db.save(&info, &items)
}

fn write_file(target: &Path, entry: &Entry, contents: &mut dyn Read, root: bool) -> Result<String> {
    use sha2::Digest;
    use std::io::Write;
    if target.is_dir() && !target.is_symlink() {
        return fail(format!("{}: a folder is in the way of a file", target.display()));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    let temporary = temporary(target);
    let mut file = std::fs::File::create(&temporary).ctx(&temporary)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = contents.read(&mut buffer).ctx(&temporary)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).ctx(&temporary)?;
    }
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(entry.mtime)).ctx(&temporary)?;
    drop(file);
    // Owner first: chown clears setuid bits.
    set_owner(&temporary, entry, root)?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(entry.mode)).ctx(&temporary)?;
    std::fs::rename(&temporary, target).ctx(target)?;
    Ok(fetch::hex(&hasher.finalize()))
}

fn set_owner(path: &Path, entry: &Entry, root: bool) -> Result<()> {
    if root {
        std::os::unix::fs::lchown(path, Some(entry.uid), Some(entry.gid)).ctx(path)?;
    }
    Ok(())
}

fn temporary(target: &Path) -> std::path::PathBuf {
    target.with_file_name(format!(".{}.autark-pkg-{}", file_name(target), std::process::id()))
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn is_config(path: &str) -> bool {
    path.starts_with("/etc/")
}

/// Refreshes the linker cache when libraries came or went. Needs root (`-r` chroots).
fn ldconfig(config: &Config, items: &[Item]) {
    if !is_root() || !items.iter().any(|item| item.path.contains("/lib") && item.path.contains(".so")) {
        return;
    }
    let mut command = Command::new("ldconfig");
    if !config.is_live() {
        command.arg("-r").arg(&config.root);
    }
    if !command.status().is_ok_and(|status| status.success()) {
        println!("   ldconfig failed; run it by hand");
    }
}
