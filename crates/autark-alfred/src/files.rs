//! Writing the daemon's files: only you can read them (files 0600 in folders 0700), and a file is replaced whole or
//! not at all, through a temporary file in the same folder, so a crash or a full disk never leaves half of one.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Makes `dir` (and what leads to it) if it isn't there, and keeps it to you.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    // A folder made before this was 0755: the daemon's own folder is narrowed, never one above it.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Replaces `path` with `bytes`: written to a temporary file made 0600 beside it, flushed to disk, then renamed over it.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| std::io::Error::other(format!("{} has no folder", path.display())))?;
    private_dir(dir)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = dir.join(format!(".{name}.saving-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        // The rename itself is kept when the folder is flushed too.
        std::fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        // Nothing to report beyond the error returned: the temporary file is only cleaned up.
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Moves a file that can't be understood out of the way, to `<name>.bad-<seconds>` beside it, so a fresh one can be
/// written without losing it. Returns where it went.
pub fn set_aside(path: &Path, now: u64) -> std::io::Result<std::path::PathBuf> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut aside = path.with_file_name(format!("{name}.bad-{now}"));
    let mut n = 2;
    while aside.exists() {
        aside = path.with_file_name(format!("{name}.bad-{now}-{n}"));
        n += 1;
    }
    std::fs::rename(path, &aside)?;
    Ok(aside)
}

/// Narrows what an earlier version left under `root` to you alone: folders 0700, files 0600. A link is left as it is and
/// not followed, so nothing outside `root` is changed. What can't be changed is logged and the rest goes on.
pub fn tighten(root: &Path) {
    let meta = match std::fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => return eprintln!("alfred: looking at {}: {err}", root.display()),
    };
    let wanted = if meta.is_dir() {
        0o700
    } else if meta.is_file() {
        0o600
    } else {
        // A link, or anything else that isn't ours to narrow.
        return;
    };
    if meta.permissions().mode() & 0o777 != wanted
        && let Err(err) = std::fs::set_permissions(root, std::fs::Permissions::from_mode(wanted))
    {
        eprintln!("alfred: narrowing {}: {err}", root.display());
    }
    if meta.is_dir() {
        match std::fs::read_dir(root) {
            Ok(entries) => entries.flatten().for_each(|entry| tighten(&entry.path())),
            Err(err) => eprintln!("alfred: reading {}: {err}", root.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_file_is_replaced_whole_and_only_yours() {
        let dir = std::env::temp_dir().join(format!("alfred-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.join("a.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!((mode(&dir), mode(&path)), (0o700, 0o600));
        // No temporary file is left beside it.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_an_earlier_version_left_open_is_narrowed_and_links_are_not_followed() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("alfred-tighten-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("state"), base.join("outside"));
        std::fs::create_dir_all(root.join("agents/piper")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let open = |path: &Path, mode: u32| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        for (file, text) in [("events.jsonl", "{}"), ("agents/piper/chat.json", "{}")] {
            std::fs::write(root.join(file), text).unwrap();
            open(&root.join(file), 0o644);
        }
        for dir in ["", "agents", "agents/piper"] {
            open(&root.join(dir), 0o755);
        }
        std::fs::write(outside.join("notes.md"), "mine").unwrap();
        open(&outside.join("notes.md"), 0o644);
        open(&outside, 0o755);
        symlink(outside.join("notes.md"), root.join("agents/piper/link.md")).unwrap();
        symlink(&outside, root.join("agents/outside")).unwrap();
        tighten(&root);
        for dir in ["", "agents", "agents/piper"] {
            assert_eq!(mode(&root.join(dir)), 0o700, "{dir}");
        }
        assert_eq!((mode(&root.join("events.jsonl")), mode(&root.join("agents/piper/chat.json"))), (0o600, 0o600));
        assert_eq!((mode(&outside), mode(&outside.join("notes.md"))), (0o755, 0o644));
        // A folder that isn't there is nothing to do.
        tighten(&base.join("missing"));
        let _ = std::fs::remove_dir_all(base);
    }
}
