//! A log that survives a hang. Lines go to the kernel log (`/dev/kmsg`) and, when the command line has `autark.log=<device>`, to a
//! file on that FAT partition (the EFI system partition), flushed to disk after every line, so a boot that freezes on a blank
//! screen still leaves a trail. Secrets are never logged: not the PIN, not its length.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

use rustix::mount::{MountFlags, UnmountFlags, mount, unmount};

static FILE: Mutex<Option<File>> = Mutex::new(None);
const MOUNT: &str = "/dbg";
const LOG: &str = "/dbg/EFI/autark-rust/init.log";

fn uptime() -> String {
    std::fs::read_to_string("/proc/uptime").ok().and_then(|text| text.split_whitespace().next().map(str::to_owned)).unwrap_or_else(|| "?".to_owned())
}

pub fn log(message: &str) {
    if let Ok(mut kmsg) = OpenOptions::new().write(true).open("/dev/kmsg") {
        let _ = kmsg.write_all(format!("<4>autark-init: {message}\n").as_bytes());
    }
    if let Ok(mut slot) = FILE.lock()
        && let Some(file) = slot.as_mut()
    {
        let _ = file.write_all(format!("[{:>8}] {message}\n", uptime()).as_bytes());
        let _ = file.sync_all();
    }
}

/// Mounts `device` (a FAT partition) and starts appending to `EFI/autark-rust/init.log` on it.
pub fn start_file(device: &str) -> Result<(), String> {
    // The disk may not have been found yet this early: wait for the partition to appear, for up to ten seconds.
    for _ in 0..100 {
        if std::path::Path::new(device).exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::fs::create_dir_all(MOUNT).map_err(|error| format!("{MOUNT}: {error}"))?;
    mount(device, MOUNT, "vfat", MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC | MountFlags::NOATIME, None).map_err(|error| format!("mounting {device}: {error}"))?;
    std::fs::create_dir_all("/dbg/EFI/autark-rust").map_err(|error| format!("{LOG}: {error}"))?;
    let file = OpenOptions::new().create(true).append(true).open(LOG).map_err(|error| format!("{LOG}: {error}"))?;
    if let Ok(mut slot) = FILE.lock() {
        *slot = Some(file);
    }
    log("--- log started");
    Ok(())
}

/// Closes the file and gives the partition back, so it is left clean.
pub fn stop_file() {
    let had = FILE.lock().ok().and_then(|mut slot| slot.take());
    if let Some(file) = had {
        let _ = file.sync_all();
        drop(file);
        let _ = unmount(MOUNT, UnmountFlags::empty());
    }
}
