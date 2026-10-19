//! Finding the block device that holds a LUKS2 volume by its UUID. There's no udev in the initramfs, so no
//! `/dev/disk/by-uuid`: this looks through `/sys/class/block` and asks libcryptsetup which header carries the UUID.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cryptsetup;

/// Names under `/sys/class/block` worth looking inside: disks and partitions, not loop, RAM, zram, device-mapper, optical or floppy devices.
fn worth_trying(name: &str) -> bool {
    !["loop", "ram", "zram", "dm-", "sr", "fd", "md", "nbd"].iter().any(|prefix| name.starts_with(prefix))
}

/// The device names to try, partitions of one disk together, in a stable order.
pub fn candidates(sys_class_block: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(sys_class_block).into_iter().flatten().flatten().filter_map(|entry| entry.file_name().into_string().ok()).filter(|name| worth_trying(name)).collect();
    found.sort();
    found
}

/// One pass over the devices: the path of the one whose LUKS2 header has this UUID.
pub fn find_once(uuid: &str) -> Option<PathBuf> {
    candidates(Path::new("/sys/class/block")).into_iter().map(|name| Path::new("/dev").join(name)).find(|path| cryptsetup::uuid_of(path).as_deref() == Some(uuid))
}

/// Looks again every quarter second until the device shows up or `timeout` passes (USB disks and slow controllers appear late).
pub fn find(uuid: &str, timeout: Duration) -> Option<PathBuf> {
    let start = Instant::now();
    loop {
        if let Some(path) = find_once(uuid) {
            return Some(path);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disks_and_partitions_are_tried_virtual_devices_are_not() {
        let root = std::env::temp_dir().join(format!("autark-init-sys-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["nvme0n1", "nvme0n1p1", "nvme0n1p3", "sda", "sda1", "loop0", "ram0", "zram0", "dm-0", "sr0", "fd0", "md127"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        assert_eq!(candidates(&root), ["nvme0n1", "nvme0n1p1", "nvme0n1p3", "sda", "sda1"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_sysfs_is_no_candidates() {
        assert!(candidates(Path::new("/nonexistent/sys")).is_empty());
    }
}
