//! File operations, run off the main thread. Names never clash: a copy or move into a folder
//! that already has the name gets "name 2", as Notes names new notes.

use std::path::{Path, PathBuf};

use autark_common::files::{self, Trashed};

pub enum Op {
    Copy(Vec<PathBuf>, PathBuf),
    Move(Vec<PathBuf>, PathBuf),
    /// The paths and the local time for the `.trashinfo` files.
    Trash(Vec<PathBuf>, String),
    Restore(Vec<Trashed>),
    Delete(Vec<Trashed>),
    /// Pack these, which share a folder, into a zip beside them, or a `tar.gz` when set (see `autark-archive`).
    Compress(Vec<PathBuf>, bool),
    /// Unpack each of these archives beside itself.
    Extract(Vec<PathBuf>),
}

#[derive(Default, Debug)]
pub struct Done {
    /// For the status bar.
    pub message: String,
    /// Where each item ended up, to select them afterwards (or to undo a trash).
    pub results: Vec<PathBuf>,
    pub errors: Vec<String>,
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// "3 items" or "“plan.md”".
fn things(paths: &[PathBuf]) -> String {
    match paths {
        [one] => format!("“{}”", name(one)),
        _ => format!("{} items", paths.len()),
    }
}

/// Whether `path` is named like something `autark-archive` can unpack.
pub fn is_archive(path: &Path) -> bool {
    let name = name(path).to_lowercase();
    [".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".7z", ".rar", ".jar"].iter().any(|ext| name.len() > ext.len() && name.ends_with(ext))
}

/// Runs `autark-archive` and returns the paths it printed, one a line; what it complained of if it failed.
fn archive_tool(args: &[&std::ffi::OsStr]) -> Result<Vec<PathBuf>, String> {
    let output = std::process::Command::new("autark-archive").args(args).output().map_err(|err| format!("autark-archive: {err}"))?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("it failed");
        return Err(line.trim_start_matches("autark-archive: ").to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().filter(|line| !line.is_empty()).map(PathBuf::from).collect())
}

/// Blocks.
pub fn run(op: Op) -> Done {
    let mut done = Done::default();
    let mut fail = |verb: &str, path: &Path, err: &dyn std::fmt::Display| done.errors.push(format!("Couldn't {verb} “{}”: {err}", name(path)));
    let mut results = Vec::new();
    let (succeeded, message) = match op {
        Op::Copy(paths, into) => {
            for path in &paths {
                if files::is_within(&into, path) {
                    fail("copy", path, &"a folder can't go inside itself");
                    continue;
                }
                let target = files::free_name(&into, &name(path));
                match files::copy_tree(path, &target) {
                    Ok(()) => results.push(target),
                    Err(err) => fail("copy", path, &err),
                }
            }
            (results.len(), format!("Copied {}", things(&results)))
        }
        Op::Move(paths, into) => {
            for path in &paths {
                if path.parent() == Some(into.as_path()) {
                    results.push(path.clone());
                    continue;
                }
                if files::is_within(&into, path) {
                    fail("move", path, &"a folder can't go inside itself");
                    continue;
                }
                let target = files::free_name(&into, &name(path));
                match files::move_tree(path, &target) {
                    Ok(()) => results.push(target),
                    Err(err) => fail("move", path, &err),
                }
            }
            (results.len(), format!("Moved {}", things(&results)))
        }
        Op::Trash(paths, now) => {
            let mut moved = Vec::new();
            for path in &paths {
                match files::trash(path, &now) {
                    Ok(target) => {
                        moved.push(path.clone());
                        results.push(target);
                    }
                    Err(err) => fail("move to the Trash", path, &err),
                }
            }
            (moved.len(), format!("Moved {} to the Trash", things(&moved)))
        }
        Op::Restore(items) => {
            for item in &items {
                match files::restore(item) {
                    Ok(target) => results.push(target),
                    Err(err) => fail("put back", &item.original, &err),
                }
            }
            (results.len(), format!("Put back {}", things(&results)))
        }
        Op::Delete(items) => {
            let mut deleted = Vec::new();
            for item in &items {
                match files::delete(item) {
                    Ok(()) => deleted.push(item.original.clone()),
                    Err(err) => fail("delete", &item.original, &err),
                }
            }
            (deleted.len(), format!("Deleted {}", things(&deleted)))
        }
        Op::Compress(paths, tar) => {
            let mut args: Vec<&std::ffi::OsStr> = vec!["compress".as_ref()];
            if tar {
                args.push("--tar".as_ref());
            }
            args.extend(paths.iter().map(|path| path.as_os_str()));
            match archive_tool(&args) {
                Ok(made) => results = made,
                Err(err) => done.errors.push(format!("Couldn't compress {}: {err}", things(&paths))),
            }
            (results.len(), format!("Compressed {} to {}", things(&paths), things(&results)))
        }
        Op::Extract(paths) => {
            for path in &paths {
                match archive_tool(&["extract".as_ref(), path.as_os_str()]) {
                    Ok(made) => results.extend(made),
                    Err(err) => fail("extract", path, &err),
                }
            }
            (results.len(), format!("Extracted {} to {}", things(&paths), things(&results)))
        }
    };
    if succeeded > 0 {
        done.message = message;
    }
    done.results = results;
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_known_by_name() {
        for yes in ["a.zip", "A.ZIP", "backup.tar.gz", "x.tgz", "y.7z", "z.tar.zst"] {
            assert!(is_archive(Path::new(yes)), "{yes}");
        }
        for no in ["notes.txt", "zip", ".zip", "tar.gz", "photo.png"] {
            assert!(!is_archive(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn packing_and_unpacking_go_through_the_archive_tool() {
        // Only where the tool is installed; the archive crate's own tests cover how it behaves.
        if std::process::Command::new("autark-archive").arg("compress").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("autark-ops-archive-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("folder/a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        let packed = run(Op::Compress(vec![dir.join("folder"), dir.join("b.txt")], false));
        assert!(packed.errors.is_empty(), "{:?}", packed.errors);
        assert_eq!(packed.results, vec![dir.join("Archive.zip")]);
        let unpacked = run(Op::Extract(packed.results.clone()));
        assert!(unpacked.errors.is_empty(), "{:?}", unpacked.errors);
        assert_eq!(unpacked.results, vec![dir.join("Archive")]);
        assert!(dir.join("Archive/folder/a.txt").exists() && dir.join("Archive/b.txt").exists());
        let bad = run(Op::Extract(vec![dir.join("b.txt")]));
        assert_eq!(bad.errors.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copies_beside_and_refuses_into_itself() {
        let dir = std::env::temp_dir().join(format!("autark-ops-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("plan.md"), "plan").unwrap();
        let done = run(Op::Copy(vec![dir.join("plan.md")], dir.clone()));
        assert_eq!(done.results, vec![dir.join("plan 2.md")]);
        assert_eq!(done.message, "Copied “plan 2.md”");
        let done = run(Op::Move(vec![dir.join("folder")], dir.join("folder")));
        assert!(done.results.is_empty());
        assert_eq!(done.errors.len(), 1);
        let done = run(Op::Move(vec![dir.join("plan.md"), dir.join("plan 2.md")], dir.join("folder")));
        assert_eq!(done.message, "Moved 2 items");
        assert!(dir.join("folder/plan 2.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
