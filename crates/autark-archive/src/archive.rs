//! Reading and writing archives through `bsdtar` (libarchive), which knows zip, tar with gzip, bzip2, xz and zstd, 7z
//! and more. Nothing here overwrites a file: extraction goes into a fresh folder beside its destination and is
//! moved into place under names that are free.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// As the archive names it; a folder ends in `/`.
    pub path: String,
    pub size: u64,
    pub dir: bool,
    pub modified: String,
}

/// One line of `bsdtar -tv`: `-rw-r--r--  0 user group  1234 Oct 19 12:00 path/to file`.
fn parse(line: &str) -> Option<Entry> {
    let mut rest = line;
    let mut field = || {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        let (word, tail) = rest.split_at(end);
        rest = tail;
        Some(word)
    };
    let permissions = field()?;
    let _links = field()?;
    let _owner = field()?;
    let _group = field()?;
    let size = field()?.parse().ok()?;
    let (month, day, when) = (field()?, field()?, field()?);
    let mut path = rest.trim_start().to_owned();
    if path.is_empty() {
        return None;
    }
    // A link is listed with what it points at.
    if permissions.starts_with('l') {
        if let Some((name, _)) = path.split_once(" -> ") {
            path = name.to_owned();
        }
    } else if permissions.starts_with('h') {
        if let Some((name, _)) = path.split_once(" link to ") {
            path = name.to_owned();
        }
    }
    Some(Entry { dir: permissions.starts_with('d') || path.ends_with('/'), path, size, modified: format!("{month} {day} {when}") })
}

fn failure(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr);
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("bsdtar failed");
    line.trim_start_matches("bsdtar: ").to_owned()
}

pub fn list(archive: &Path) -> Result<Vec<Entry>, String> {
    let output = Command::new("bsdtar").arg("-tvf").arg(archive).output().map_err(|error| format!("bsdtar: {error}"))?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().filter_map(parse).collect())
}

/// The first part of each path, once: what unpacking puts down at the top.
pub fn top_level(entries: &[Entry]) -> Vec<String> {
    let mut tops: Vec<String> = Vec::new();
    for entry in entries {
        let path = entry.path.trim_start_matches("./");
        if let Some(top) = path.split('/').find(|part| !part.is_empty()) {
            if !tops.iter().any(|known| known == top) {
                tops.push(top.to_owned());
            }
        }
    }
    tops
}

/// What an archive's folder is called when it has none of its own: its file name without the archive extensions.
pub fn stem(archive: &Path) -> String {
    let name = archive.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    for extension in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz", ".tgz", ".tbz2", ".txz", ".tar", ".zip", ".7z", ".rar", ".gz", ".bz2", ".xz", ".zst", ".jar"] {
        if lower.ends_with(extension) && name.len() > extension.len() {
            return name[..name.len() - extension.len()].to_owned();
        }
    }
    name
}

/// Takes `selected` (everything, when empty) out of `archive` into `dest`, and returns what was put there. Everything
/// comes out into a hidden folder in `dest` first: a lone top-level folder is moved up as it is, loose files go into
/// a folder named for the archive, and whatever is already there is left alone, as the new things take free names.
pub fn extract(archive: &Path, dest: &Path, selected: &[String], entries: &[Entry]) -> Result<Vec<PathBuf>, String> {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.subsec_nanos());
    let work = dest.join(format!(".autark-extracting-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", dest.display()))?;
    let result = (|| {
        let mut command = Command::new("bsdtar");
        command.arg("-xf").arg(archive).arg("-C").arg(&work);
        if !selected.is_empty() {
            command.arg("--").args(selected);
        }
        let output = command.output().map_err(|error| format!("bsdtar: {error}"))?;
        if !output.status.success() {
            return Err(failure(&output));
        }
        let mut made = Vec::new();
        let mut found: Vec<PathBuf> = std::fs::read_dir(&work).map_err(|error| error.to_string())?.flatten().map(|entry| entry.path()).collect();
        found.sort();
        let whole = selected.is_empty();
        if whole && found.len() > 1 && top_level(entries).len() > 1 {
            // Loose files: into a folder of their own, so they don't spill into the one that holds the archive.
            let folder = autark_common::files::free_name(dest, &stem(archive));
            std::fs::create_dir(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
            for path in &found {
                let target = folder.join(path.file_name().unwrap_or_default());
                std::fs::rename(path, &target).map_err(|error| format!("{}: {error}", target.display()))?;
            }
            made.push(folder);
        } else {
            for path in &found {
                let target = autark_common::files::free_name(dest, &path.file_name().unwrap_or_default().to_string_lossy());
                std::fs::rename(path, &target).map_err(|error| format!("{}: {error}", target.display()))?;
                made.push(target);
            }
        }
        Ok(made)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Packs `files` (all in one folder) into an archive beside them: a zip, or `tar.gz` when `tar` is set. It's named for
/// the file when there's one, and "Archive" when there are several, and takes a free name.
pub fn compress(files: &[PathBuf], tar: bool) -> Result<PathBuf, String> {
    let first = files.first().ok_or("nothing to pack")?;
    let parent = first.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    if files.iter().any(|file| file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new(".")) != parent) {
        return Err("pack files from one folder at a time".to_owned());
    }
    let base = if files.len() == 1 { first.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default() } else { "Archive".to_owned() };
    let out = autark_common::files::free_name(parent, &format!("{base}.{}", if tar { "tar.gz" } else { "zip" }));
    let mut command = Command::new("bsdtar");
    command.arg("-a").arg("-cf").arg(&out).arg("-C").arg(parent);
    for file in files {
        command.arg(file.file_name().ok_or("a file without a name")?);
    }
    let output = command.output().map_err(|error| format!("bsdtar: {error}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(failure(&output));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder that goes when the test does.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("autark-archive-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn lines_become_entries() {
        let entry = parse("-rw-r--r--  0 tan    users    1234 Oct 19 12:00 my notes/a file.txt").unwrap();
        assert_eq!((entry.path.as_str(), entry.size, entry.dir, entry.modified.as_str()), ("my notes/a file.txt", 1234, false, "Oct 19 12:00"));
        assert!(parse("drwxr-xr-x  0 tan users 0 Oct 19 12:00 folder/").unwrap().dir);
        assert_eq!(parse("lrwxr-xr-x  0 tan users 0 Oct 19 12:00 here -> there").unwrap().path, "here");
        assert!(parse("not an entry").is_none());
    }

    #[test]
    fn names_lose_their_archive_extensions() {
        assert_eq!(stem(Path::new("/x/photos.tar.gz")), "photos");
        assert_eq!(stem(Path::new("/x/Photos.ZIP")), "Photos");
        assert_eq!(stem(Path::new("/x/notes.txt")), "notes.txt");
        assert_eq!(stem(Path::new("/x/.zip")), ".zip");
    }

    #[test]
    fn pack_list_and_unpack_round_trip_without_overwriting() {
        let scratch = Scratch::new("round");
        let src = scratch.0.join("src");
        std::fs::create_dir_all(src.join("pics")).unwrap();
        std::fs::write(src.join("a.txt"), "alpha").unwrap();
        std::fs::write(src.join("pics/b file.txt"), "beta").unwrap();

        for tar in [false, true] {
            let archive = compress(&[src.join("a.txt"), src.join("pics")], tar).unwrap();
            assert!(archive.file_name().unwrap().to_string_lossy().starts_with("Archive."));
            let entries = list(&archive).unwrap();
            let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
            assert!(paths.contains(&"a.txt") && paths.contains(&"pics/b file.txt"), "{paths:?}");
            assert_eq!(top_level(&entries), ["a.txt", "pics"]);

            // Several things at the top go into a folder named for the archive.
            let out = scratch.0.join(if tar { "out-tar" } else { "out-zip" });
            let made = extract(&archive, &out, &[], &entries).unwrap();
            assert_eq!(made.len(), 1);
            assert_eq!(std::fs::read_to_string(made[0].join("pics/b file.txt")).unwrap(), "beta");
            // Doing it again leaves the first alone and makes a second.
            let again = extract(&archive, &out, &[], &entries).unwrap();
            assert_ne!(made, again);
            assert!(made[0].join("a.txt").exists() && again[0].join("a.txt").exists());
            // No working folder is left behind.
            assert!(std::fs::read_dir(&out).unwrap().flatten().all(|entry| !entry.file_name().to_string_lossy().starts_with('.')));
        }
    }

    #[test]
    fn a_lone_folder_is_not_wrapped_again_and_a_selection_comes_out_alone() {
        let scratch = Scratch::new("lone");
        let src = scratch.0.join("src");
        std::fs::create_dir_all(src.join("project")).unwrap();
        std::fs::write(src.join("project/one.txt"), "1").unwrap();
        std::fs::write(src.join("project/two.txt"), "2").unwrap();
        let archive = compress(&[src.join("project")], false).unwrap();
        let entries = list(&archive).unwrap();
        let out = scratch.0.join("out");
        let made = extract(&archive, &out, &[], &entries).unwrap();
        assert_eq!(made, [out.join("project")]);
        assert!(out.join("project/one.txt").exists());

        let only = scratch.0.join("only");
        let picked = extract(&archive, &only, &["project/two.txt".to_owned()], &entries).unwrap();
        assert_eq!(picked.len(), 1);
        assert!(only.join("project/two.txt").exists() && !only.join("project/one.txt").exists());
    }

    #[test]
    fn a_lone_file_comes_out_as_a_file() {
        let scratch = Scratch::new("file");
        std::fs::write(scratch.0.join("note.txt"), "text").unwrap();
        let archive = compress(&[scratch.0.join("note.txt")], true).unwrap();
        let entries = list(&archive).unwrap();
        let out = scratch.0.join("out");
        assert_eq!(extract(&archive, &out, &[], &entries).unwrap(), [out.join("note.txt")]);
        assert_eq!(extract(&archive, &out, &[], &entries).unwrap(), [out.join("note 2.txt")]);
    }

    #[test]
    fn a_bad_archive_says_so() {
        let scratch = Scratch::new("bad");
        let bad = scratch.0.join("nothing.zip");
        std::fs::write(&bad, "this is not an archive").unwrap();
        assert!(list(&bad).is_err());
        assert!(compress(&[], false).is_err());
    }
}
