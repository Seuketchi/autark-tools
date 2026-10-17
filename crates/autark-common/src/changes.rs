//! Watching config files from a daemon, which has no GTK main loop: inotify on the folders that hold
//! `~/.config/autark/<name>.toml` and `/etc/autark/<name>.toml`, waited on with async-io. Folders are
//! watched rather than files, so a file that's replaced (as editors and Settings save) or made for the first
//! time is still seen.

use std::ffi::OsString;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::PathBuf;

use async_io::Async;
use rustix::fs::inotify;

pub struct Changes {
    fd: Async<OwnedFd>,
    /// The file names that count; the folders may hold others.
    names: Vec<OsString>,
}

impl Changes {
    /// Watches `name`'s config files. The user's folder is made if it isn't there, so the first save is seen.
    pub fn config(name: &str) -> std::io::Result<Self> {
        Self::files(&crate::config::paths(name))
    }

    pub fn files(paths: &[PathBuf]) -> std::io::Result<Self> {
        let fd = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)?;
        let flags = inotify::WatchFlags::CLOSE_WRITE | inotify::WatchFlags::MOVED_TO | inotify::WatchFlags::CREATE | inotify::WatchFlags::DELETE | inotify::WatchFlags::MOVED_FROM;
        let mut names = Vec::new();
        for path in paths {
            let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else { continue };
            if !folder.starts_with("/etc") {
                let _ = std::fs::create_dir_all(folder);
            }
            // A folder that isn't there (no /etc/autark yet) is left out.
            if inotify::add_watch(&fd, folder, flags).is_ok() {
                names.push(name.to_owned());
            }
        }
        Ok(Self { fd: Async::new_nonblocking(fd)?, names })
    }

    /// Waits until one of the files changes. Several changes close together come back as one.
    pub async fn next(&mut self) -> std::io::Result<()> {
        loop {
            self.fd.readable().await?;
            let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
            let mut changed = false;
            let mut reader = inotify::Reader::new(self.fd.get_ref(), &mut buffer);
            loop {
                match reader.next() {
                    Ok(event) => {
                        let name = event.file_name().map(|name| OsString::from(name.to_string_lossy().into_owned()));
                        changed |= name.is_some_and(|name| self.names.contains(&name));
                    }
                    Err(rustix::io::Errno::AGAIN) => break,
                    Err(error) => return Err(error.into()),
                }
            }
            if changed {
                // Let a save that writes in steps finish first.
                async_io::Timer::after(std::time::Duration::from_millis(120)).await;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_file_is_seen() {
        let folder = std::env::temp_dir().join(format!("autark-changes-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("power.toml");
        let mut changes = Changes::files(std::slice::from_ref(&path)).unwrap();
        std::fs::write(folder.join("other.toml"), "x = 1").unwrap();
        let saved = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::write(&saved, "low_battery = 15").unwrap();
        });
        let seen = async_io::block_on(async { futures_lite_timeout(changes.next()).await });
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(seen, "the change came within a second");
    }

    async fn futures_lite_timeout(changed: impl std::future::Future<Output = std::io::Result<()>>) -> bool {
        let timeout = async {
            async_io::Timer::after(std::time::Duration::from_secs(1)).await;
            false
        };
        let changed = async { changed.await.is_ok() };
        futures_lite::future::or(changed, timeout).await
    }
}
