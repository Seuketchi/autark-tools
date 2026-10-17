//! Locations: the system disk and whatever is mounted under `/run/media`, `/media` or `/mnt`.
//! Mounting by click waits for UDisks2; Eject unmounts what's already there.

use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    pub name: String,
    pub path: PathBuf,
    pub filesystem: String,
    /// The system disk can't be ejected.
    pub removable: bool,
}

pub fn mounted() -> Vec<Drive> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    parse(&text, &system_name())
}

/// The system's name from os-release, "Autark" on Autark.
fn system_name() -> String {
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    text.lines().find_map(|line| line.strip_prefix("NAME=")).map(|name| name.trim_matches('"').to_owned()).unwrap_or_else(|| "Computer".to_owned())
}

fn parse(mountinfo: &str, system: &str) -> Vec<Drive> {
    let mut drives = Vec::new();
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let Some(separator) = fields.iter().position(|field| *field == "-") else { continue };
        let (Some(mount), Some(filesystem)) = (fields.get(4), fields.get(separator + 1)) else { continue };
        let path = PathBuf::from(unescape(mount));
        let removable = ["/run/media/", "/media/", "/mnt/"].iter().any(|prefix| path.starts_with(prefix) && path != Path::new(prefix.trim_end_matches('/')));
        if path != Path::new("/") && !removable {
            continue;
        }
        let name = if removable { path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()) } else { system.to_owned() };
        if !drives.iter().any(|drive: &Drive| drive.path == path) {
            drives.push(Drive { name, path, filesystem: (*filesystem).to_owned(), removable });
        }
    }
    drives
}

/// mountinfo writes spaces and the like as `\040`.
fn unescape(field: &str) -> String {
    let mut out = String::new();
    let mut rest = field;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        match rest.get(at + 1..at + 4).and_then(|octal| u8::from_str_radix(octal, 8).ok()) {
            Some(byte) => {
                out.push(byte as char);
                rest = &rest[at + 4..];
            }
            None => {
                out.push('\\');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Bytes free on the disk holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let info = gio::File::for_path(path).query_filesystem_info("filesystem::free", gio::Cancellable::NONE).ok()?;
    Some(info.attribute_uint64("filesystem::free"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_system_disk_and_removable_ones() {
        let mountinfo = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
23 22 0:5 / /proc rw - proc proc rw
40 22 8:17 / /run/media/tan/USB\\040STICK rw,nosuid shared:9 - vfat /dev/sdb1 rw
41 22 8:18 / /mnt rw - ext4 /dev/sdc1 rw
";
        let drives = parse(mountinfo, "Autark");
        assert_eq!(drives.len(), 2);
        assert_eq!((drives[0].name.as_str(), drives[0].filesystem.as_str(), drives[0].removable), ("Autark", "ext4", false));
        assert_eq!(drives[1].name, "USB STICK");
        assert_eq!(drives[1].path, PathBuf::from("/run/media/tan/USB STICK"));
        assert!(drives[1].removable);
    }
}
