//! File code shared by Files, Notes and the widgets: atomic saves, free names, copying and
//! moving whole trees, and the freedesktop Trash (so files trashed here show up in any other
//! app's Trash, and the other way round).

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Writes through a temporary file beside `path`, so a reader never sees half a file.
pub fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.saving"));
    std::fs::write(&temporary, contents)?;
    std::fs::rename(&temporary, path)
}

/// `name` in `dir` if it's free, otherwise "name 2", "name 3"… with any extension kept after
/// the number ("photo 2.jpg").
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    (1..)
        .map(|n| if n == 1 { dir.join(name) } else { dir.join(numbered(name, n)) })
        .find(|path| path.symlink_metadata().is_err())
        .expect("an endless range")
}

/// Copies a file, a symlink (as a link) or a whole folder, keeping permissions and times.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = from.symlink_metadata()?;
    if metadata.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)?;
    } else if metadata.is_dir() {
        std::fs::create_dir(to)?;
        for child in std::fs::read_dir(from)? {
            let child = child?;
            copy_tree(&child.path(), &to.join(child.file_name()))?;
        }
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(metadata.mode()))?;
    } else {
        std::fs::copy(from, to)?;
        if let Ok(modified) = metadata.modified() {
            let _ = std::fs::File::options().write(true).open(to).and_then(|file| file.set_modified(modified));
        }
    }
    Ok(())
}

/// Renames, or copies and then removes when `to` is on another disk.
pub fn move_tree(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            copy_tree(from, to)?;
            remove_tree(from)
        }
        result => result,
    }
}

pub fn remove_tree(path: &Path) -> io::Result<()> {
    if path.symlink_metadata()?.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}

/// Whether `inner` is `outer` or somewhere inside it: a folder can't go into itself.
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    inner.starts_with(outer)
}

/// One trashed item.
#[derive(Clone, Debug, PartialEq)]
pub struct Trashed {
    /// Where it is now, in the Trash's `files` folder.
    pub path: PathBuf,
    /// Where it came from.
    pub original: PathBuf,
    /// As the `.trashinfo` file says: local time, `2026-10-17T18:30:00`.
    pub deleted: String,
    info: PathBuf,
}

/// The home Trash, `$XDG_DATA_HOME/Trash`.
pub fn home_trash() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("Trash")
}

fn uid() -> u32 {
    std::fs::metadata("/proc/self").map(|metadata| metadata.uid()).unwrap_or(0)
}

/// The Trash for `path`: the home one on the home disk, otherwise `.Trash-<uid>` at the top of
/// the disk it's on, as the spec says.
fn trash_for(path: &Path) -> io::Result<PathBuf> {
    let home = home_trash();
    let device = path.symlink_metadata()?.dev();
    let home_device = home.ancestors().find_map(|dir| dir.metadata().ok()).map(|metadata| metadata.dev());
    if home_device == Some(device) {
        return Ok(home);
    }
    let mut top = path.parent().unwrap_or(path).to_path_buf();
    while let Some(parent) = top.parent() {
        if parent.metadata().map(|metadata| metadata.dev()).ok() != Some(device) {
            break;
        }
        top = parent.to_path_buf();
    }
    Ok(top.join(format!(".Trash-{}", uid())))
}

/// Moves `path` to its Trash. `now` is the local time for the `.trashinfo` file.
pub fn trash(path: &Path, now: &str) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let trash = trash_for(&path)?;
    let (files, info) = (trash.join("files"), trash.join("info"));
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).ok_or_else(|| io::Error::other("nothing to trash"))?;
    // The info file is made first and exclusively: that's how the spec claims a name.
    let mut n = 1;
    let (target, info_file) = loop {
        let candidate = if n == 1 { name.clone() } else { numbered(&name, n) };
        let info_file = info.join(format!("{candidate}.trashinfo"));
        match std::fs::File::options().write(true).create_new(true).open(&info_file) {
            Ok(mut file) => {
                use std::io::Write;
                write!(file, "[Trash Info]\nPath={}\nDeletionDate={now}\n", escape(&path))?;
                break (files.join(&candidate), info_file);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(err) => return Err(err),
        }
    };
    if let Err(err) = move_tree(&path, &target) {
        let _ = std::fs::remove_file(&info_file);
        return Err(err);
    }
    Ok(target)
}

/// "name 2.ext", as `free_name` numbers.
fn numbered(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} {n}{}", &name[..dot], &name[dot..]),
        _ => format!("{name} {n}"),
    }
}

/// Everything in the home Trash and the Trash folders of mounted disks, newest first.
pub fn trashed(mounts: &[PathBuf]) -> Vec<Trashed> {
    let uid = uid();
    let mut trashes = vec![home_trash()];
    trashes.extend(mounts.iter().map(|mount| mount.join(format!(".Trash-{uid}"))));
    let mut found = Vec::new();
    for trash in trashes {
        let Ok(entries) = std::fs::read_dir(trash.join("info")) else { continue };
        for entry in entries.flatten() {
            let info = entry.path();
            let Some(name) = info.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_suffix(".trashinfo")) else { continue };
            let Ok(text) = std::fs::read_to_string(&info) else { continue };
            let (mut original, mut deleted) = (None, String::new());
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("Path=") {
                    let value = unescape(value);
                    // Relative paths are relative to the disk the Trash is on.
                    original = Some(if value.starts_with('/') { PathBuf::from(value) } else { trash.parent().unwrap_or(&trash).join(value) });
                } else if let Some(value) = line.strip_prefix("DeletionDate=") {
                    deleted = value.to_owned();
                }
            }
            let path = trash.join("files").join(name);
            if let Some(original) = original
                && path.symlink_metadata().is_ok()
            {
                found.push(Trashed { path, original, deleted, info });
            }
        }
    }
    found.sort_by(|a, b| b.deleted.cmp(&a.deleted));
    found
}

/// Puts an item back where it was; under a free name if that's taken now. Returns where it went.
pub fn restore(item: &Trashed) -> io::Result<PathBuf> {
    let parent = item.original.parent().ok_or_else(|| io::Error::other("no folder to restore into"))?;
    std::fs::create_dir_all(parent)?;
    let name = item.original.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let target = free_name(parent, &name);
    move_tree(&item.path, &target)?;
    let _ = std::fs::remove_file(&item.info);
    Ok(target)
}

/// Deletes an item for good.
pub fn delete(item: &Trashed) -> io::Result<()> {
    remove_tree(&item.path)?;
    let _ = std::fs::remove_file(&item.info);
    Ok(())
}

/// The spec's `Path=` value: the path with anything outside the safe set percent-encoded.
fn escape(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~!$&'()*+,;=:@".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("autark-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn free_names_keep_the_extension() {
        let dir = scratch("free");
        assert_eq!(free_name(&dir, "photo.jpg"), dir.join("photo.jpg"));
        std::fs::write(dir.join("photo.jpg"), "").unwrap();
        std::fs::write(dir.join("photo 2.jpg"), "").unwrap();
        assert_eq!(free_name(&dir, "photo.jpg"), dir.join("photo 3.jpg"));
        std::fs::create_dir(dir.join("untitled folder")).unwrap();
        assert_eq!(free_name(&dir, "untitled folder"), dir.join("untitled folder 2"));
        assert_eq!(free_name(&dir, ".bashrc"), dir.join(".bashrc"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copies_trees_with_links() {
        let dir = scratch("copy");
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/file"), "text").unwrap();
        std::os::unix::fs::symlink("b/file", dir.join("a/link")).unwrap();
        copy_tree(&dir.join("a"), &dir.join("c")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("c/b/file")).unwrap(), "text");
        assert_eq!(std::fs::read_link(dir.join("c/link")).unwrap(), Path::new("b/file"));
        move_tree(&dir.join("c"), &dir.join("d")).unwrap();
        assert!(dir.join("d/b/file").exists() && !dir.join("c").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn trash_and_restore() {
        let dir = scratch("trash");
        // Unsafe only with other threads reading the environment; these tests don't.
        unsafe { std::env::set_var("XDG_DATA_HOME", dir.join("data")) };
        let file = dir.join("notes 100%.md");
        std::fs::write(&file, "keep me").unwrap();
        let trashed_to = trash(&file, "2026-10-17T18:30:00").unwrap();
        assert!(!file.exists() && trashed_to.exists());
        let info = std::fs::read_to_string(dir.join("data/Trash/info/notes 100%.md.trashinfo")).unwrap();
        assert!(info.contains("notes%20100%25.md"), "{info}");

        // A second file of the same name gets a numbered slot.
        std::fs::write(&file, "second").unwrap();
        trash(&file, "2026-10-17T18:31:00").unwrap();
        let items = trashed(&[]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].original, file);
        assert_eq!(items[0].path.file_name().unwrap(), "notes 100% 2.md");

        assert_eq!(restore(&items[1]).unwrap(), file);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
        delete(&items[0]).unwrap();
        assert!(trashed(&[]).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
