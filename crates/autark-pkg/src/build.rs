//! Building: the recipe's `build()` in a clean environment, after `lib.sh`. A staged build runs as the
//! build user and installs into `$DESTDIR`; a tracked build runs as root against the system itself, and the
//! files it created or changed are found by their change time. Either way the result is a signed archive.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use crate::archive::{self, PkgInfo};
use crate::config::Config;
use crate::db;
use crate::error::{Context, Result, fail};
use crate::recipe::{Mode, Recipe};

/// Unpacks the first source (if it's an archive) and runs `build()` inside it, like the phase scripts did.
const DRIVER: &str = r#"set -euo pipefail
umask 022
cd "$srcdir"
[ ! -e "$RECIPES/lib.sh" ] || source "$RECIPES/lib.sh"
source "${RECIPE_FILE:-$RECIPES/$PACKAGE.sh}"
case "${1:-}" in
  *.tar.*|*.tgz|*.tbz2)
    # The first entry's top folder, which OpenSSH lists without a slash. sed reads all of tar's output: head
    # would kill tar with SIGPIPE, which pipefail makes fatal.
    dir=$(tar -tf "$1" | sed -n '1{s|^\./||;s|/.*||;p}')
    [ -n "$dir" ] || { echo "can't tell $1's top folder"; exit 1; }
    tar -xf "$1"
    cd "$dir" ;;
esac
build
"#;

/// Paths a tracked build's scan never looks in: other filesystems' mount points, scratch space, logs, and
/// autark-pkg's own folders.
const SKIP: &[&str] =
    &["/proc", "/sys", "/dev", "/run", "/tmp", "/var/tmp", "/var/log", "/var/cache", "/var/lib/autark", "/root", "/home", "/mnt", "/media", "/sources", "/lost+found", "/etc/ld.so.cache"];

pub struct Built {
    pub archive: PathBuf,
    /// A tracked build's files are already in place.
    pub installed: bool,
}

/// `owners` is who owns each path now: a tracked build only claims files that are new or already a package's.
pub fn build(config: &Config, recipe: &Recipe, sources: &[PathBuf], owners: &HashMap<String, String>) -> Result<Built> {
    let live_root = config.is_live() && is_root();
    if recipe.mode == Mode::Tracked && config.is_live() && !is_root() {
        return fail(format!("{} is a tracked recipe: it installs as it builds, so it needs root", recipe.name));
    }
    let work = config.builds.join(format!("{}-{}", recipe.name, recipe.full_version()));
    if work.exists() {
        std::fs::remove_dir_all(&work).ctx(&work)?;
    }
    let src = work.join("src");
    let dest = work.join("dest");
    std::fs::create_dir_all(&src).ctx(&src)?;
    for source in sources {
        let link = src.join(source.file_name().unwrap_or_default());
        std::os::unix::fs::symlink(source, &link).ctx(&link)?;
    }
    let driver = work.join("driver.sh");
    std::fs::write(&driver, DRIVER).ctx(&driver)?;
    std::fs::create_dir_all(&config.logs).ctx(&config.logs)?;
    let log_path = config.logs.join(format!("{}.log", recipe.name));
    let log = File::create(&log_path).ctx(&log_path)?;
    let failures = work.join("test-failures.txt");

    let mut command = Command::new("bash");
    if recipe.mode == Mode::Staged && live_root {
        let Some((uid, gid)) = user(&config.build_user) else {
            return fail(format!("no user {}: create it (useradd -r -U -d /var/tmp/autark-pkg -s /usr/bin/false {}) so builds don't run as root", config.build_user, config.build_user));
        };
        std::fs::create_dir_all(&dest).ctx(&dest)?;
        chown_tree(&work, uid, gid)?;
        // SAFETY: drop_to makes only async-signal-safe calls, as is required between fork and exec.
        unsafe { command.pre_exec(move || drop_to(uid, gid)) };
    } else if recipe.mode == Mode::Staged {
        std::fs::create_dir_all(&dest).ctx(&dest)?;
    }
    command.arg(&driver).args(sources.first().filter(|first| is_archive(first)).map(|first| first.file_name().unwrap_or_default().to_owned()));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/usr/sbin:/bin:/sbin")
        .env("HOME", &work)
        .env("TERM", "dumb")
        .env("LC_ALL", "C.UTF-8")
        .env("srcdir", &src)
        .env("RECIPES", &config.recipes)
        .env("RECIPE_FILE", recipe.file.as_deref().unwrap_or(Path::new("")))
        .env("PACKAGE", &recipe.name)
        .env("LOGS", &work)
        .env("ROOT", &config.root)
        .stdin(Stdio::null())
        .stdout(log.try_clone().ctx(&log_path)?)
        .stderr(log);
    if recipe.mode == Mode::Staged {
        command.env("DESTDIR", &dest);
    }

    println!("== {} {}: building ({}), log in {}", recipe.name, recipe.full_version(), recipe.mode.name(), log_path.display());
    let started = SystemTime::now();
    let before = if recipe.mode == Mode::Tracked { existing(config)? } else { HashSet::new() };
    let status = command.status().ctx("bash")?;
    if !status.success() {
        outcome(&log_path, &format!("{} {} failed ({status})", recipe.name, recipe.full_version()));
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let tail: Vec<&str> = text.lines().rev().take(25).collect();
        for line in tail.into_iter().rev() {
            println!("   | {line}");
        }
        return fail(format!("{}: build failed ({status}); the build folder {} is kept", recipe.name, work.display()));
    }
    if let Ok(text) = std::fs::read_to_string(&failures) {
        print!("{text}");
        let all = config.logs.join("test-failures.txt");
        let mut previous = std::fs::read_to_string(&all).unwrap_or_default();
        previous.push_str(&text);
        std::fs::write(&all, previous).ctx(&all)?;
    }
    let taken = started.elapsed().map_or(0, |taken| taken.as_secs());
    println!("   built in {taken}s");
    outcome(&log_path, &format!("{} {} built in {taken}s", recipe.name, recipe.full_version()));

    let info = PkgInfo { name: recipe.name.clone(), version: recipe.full_version(), depends: recipe.depends.clone(), mode: recipe.mode.name().into(), built: db::now() };
    std::fs::create_dir_all(&config.cache).ctx(&config.cache)?;
    let out = config.cache.join(info.file_name());
    let installed = match recipe.mode {
        Mode::Staged => {
            let paths = archive::tree(&dest)?;
            if !paths.iter().any(|path| !std::fs::symlink_metadata(crate::config::under(&dest, Path::new(path))).is_ok_and(|meta| meta.is_dir())) {
                return fail(format!("{}: build() installed nothing into $DESTDIR", recipe.name));
            }
            if let Some(path) = inside_itself(&paths, &dest) {
                return fail(format!("{}: the build installed under its own staging folder ({path}): $DESTDIR was applied twice, for example a Makefile that takes DESTDIR and prefix=$DESTDIR/usr", recipe.name));
            }
            archive::pack(config, &dest, &paths, &info, true, &out)?;
            false
        }
        Mode::Tracked => {
            let paths = changed(config, started, &before, owners)?;
            if paths.is_empty() {
                return fail(format!("{}: build() changed no files", recipe.name));
            }
            println!("   {} new or changed paths found", paths.len());
            archive::pack(config, &config.root, &paths, &info, false, &out)?;
            true
        }
    };
    std::fs::remove_dir_all(&work).ctx(&work)?;
    Ok(Built { archive: out, installed })
}

fn is_archive(path: &Path) -> bool {
    let name = path.to_string_lossy();
    name.contains(".tar.") || name.ends_with(".tgz") || name.ends_with(".tbz2")
}

/// A path in the staged tree that starts with the staging folder's own path: files that went to `$DESTDIR$DESTDIR/...`.
fn inside_itself<'a>(paths: &'a [String], dest: &Path) -> Option<&'a str> {
    let nested = dest.to_string_lossy();
    let nested = nested.trim_matches('/');
    paths.iter().map(String::as_str).find(|path| !nested.is_empty() && path.trim_start_matches('/').starts_with(nested))
}

pub fn is_root() -> bool {
    std::fs::metadata("/proc/self").is_ok_and(|meta| meta.uid() == 0)
}

/// Becomes the build user in the child before bash starts, as `setpriv --reuid --regid --clear-groups --no-new-privs`
/// did (Autark's util-linux is built without setpriv): no supplementary groups, real, effective and saved ids all the
/// user's, and no setuid program can raise them again.
pub fn drop_to(uid: u32, gid: u32) -> std::io::Result<()> {
    let check = |rc: libc::c_int| if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) };
    // Groups and gid first: once the uid is dropped neither can change.
    // SAFETY: plain system calls with valid arguments.
    unsafe {
        check(libc::setgroups(0, std::ptr::null()))?;
        check(libc::setresgid(gid, gid, gid))?;
        check(libc::setresuid(uid, uid, uid))?;
        check(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0))
    }
}

/// The uid and gid of a user in this system's passwd.
pub fn user(name: &str) -> Option<(u32, u32)> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() > 3 && fields[0] == name).then(|| Some((fields[2].parse().ok()?, fields[3].parse().ok()?))).flatten()
    })
}

fn chown_tree(dir: &Path, uid: u32, gid: u32) -> Result<()> {
    std::os::unix::fs::lchown(dir, Some(uid), Some(gid)).ctx(dir)?;
    for entry in std::fs::read_dir(dir).ctx(dir)? {
        let path = entry.ctx(dir)?.path();
        if std::fs::symlink_metadata(&path).ctx(&path)?.is_dir() {
            chown_tree(&path, uid, gid)?;
        } else {
            std::os::unix::fs::lchown(&path, Some(uid), Some(gid)).ctx(&path)?;
        }
    }
    Ok(())
}

/// Calls `each` for every path on the root's filesystem the scan looks at.
fn scan(config: &Config, each: &mut dyn FnMut(&Path, &std::fs::Metadata)) -> Result<()> {
    let device = std::fs::metadata(&config.root).ctx(&config.root)?.dev();
    let skip: Vec<PathBuf> = SKIP.iter().map(|path| config.path(path)).chain(config.own.iter().map(|path| config.path(&path.to_string_lossy()))).collect();
    let mut stack = vec![config.root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if skip.contains(&path) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() && meta.dev() == device {
                stack.push(path.clone());
            }
            each(&path, &meta);
        }
    }
    Ok(())
}

fn existing(config: &Config) -> Result<HashSet<PathBuf>> {
    let mut out = HashSet::new();
    scan(config, &mut |path, _| {
        out.insert(path.to_path_buf());
    })?;
    Ok(out)
}

/// Files and symlinks created or changed since `since` (their ctime moves on any write, rename or chmod,
/// which catches files a `make install` overwrote as well as new ones), plus folders that are new. A file that
/// was already there and no package owns (/etc/group after a `groupadd`) is reported, not claimed: removing
/// the package would delete it.
fn changed(config: &Config, since: SystemTime, before: &HashSet<PathBuf>, owners: &HashMap<String, String>) -> Result<Vec<String>> {
    // Inode times come from a coarse clock that can trail the real one by a tick: start a little early.
    let since = since.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |at| at.as_nanos() as i128) - 2_000_000_000;
    let (mut out, mut unclaimed) = (Vec::new(), Vec::new());
    scan(config, &mut |path, meta| {
        let name = format!("/{}", path.strip_prefix(&config.root).unwrap_or(path).to_string_lossy());
        let existed = before.contains(path);
        if meta.is_dir() {
            if !existed {
                out.push(name);
            }
            return;
        }
        let ctime = i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec());
        if !(meta.is_file() || meta.file_type().is_symlink()) || ctime < since {
            return;
        }
        if !existed || owners.contains_key(&name) {
            out.push(name);
        } else {
            unclaimed.push(name);
        }
    })?;
    unclaimed.sort();
    for name in unclaimed {
        println!("   changed but not claimed (it was there before and no package owns it): {name}");
    }
    out.sort();
    Ok(out)
}

/// Ends the log with how the build went, the line Kith's suggestions watch the log folder for:
/// `== autark-pkg: gcc 15.2.0 built in 812s`.
fn outcome(log: &Path, text: &str) {
    use std::io::Write;
    let written = std::fs::OpenOptions::new().append(true).open(log).and_then(|mut file| writeln!(file, "== autark-pkg: {text}"));
    if let Err(err) = written {
        eprintln!("autark-pkg: {}: {err}", log.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_staged_under_the_staging_folder_itself_are_found() {
        let dest = Path::new("/var/tmp/autark-pkg/hexedit-1.6-3/dest");
        let fine = ["/usr/".to_owned(), "/usr/bin/hexedit".to_owned()];
        let doubled = ["/var/".to_owned(), "/var/tmp/autark-pkg/hexedit-1.6-3/dest/usr/bin/hexedit".to_owned()];
        assert_eq!(inside_itself(&fine, dest), None);
        assert_eq!(inside_itself(&doubled, dest), Some("/var/tmp/autark-pkg/hexedit-1.6-3/dest/usr/bin/hexedit"));
    }
}
