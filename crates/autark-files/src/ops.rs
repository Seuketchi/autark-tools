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
