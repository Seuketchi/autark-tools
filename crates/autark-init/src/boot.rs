//! What an init does around the unlock: mount the virtual file systems, give itself a console, mount the unlocked root,
//! and hand over to the real init (systemd) in the way `switch_root` does. Everything here changes the running system, so none
//! of it runs unless this is PID 1 (see `main.rs`); the pure parts (which flags are generic, which init to run) are tested.

use std::convert::Infallible;
use std::ffi::CString;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::Path;

use rustix::fs::{Mode, OFlags};
use rustix::mount::{MountFlags, mount, mount_move};
use rustix::process::{chdir, chroot};

use crate::cmdline::Cmdline;

pub const SYSROOT: &str = "/sysroot";
const RAMFS_MAGIC: u64 = 0x8584_58f6;
const TMPFS_MAGIC: u64 = 0x0102_1994;
/// File systems tried for root when `rootfstype=` isn't given, in the order they're likely.
const ROOT_TYPES: [&str; 4] = ["ext4", "btrfs", "xfs", "f2fs"];

pub fn mount_special() -> Result<(), String> {
    let plain = MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC;
    for (source, target, kind, flags, data) in [
        ("proc", "/proc", "proc", plain, None),
        ("sysfs", "/sys", "sysfs", plain, None),
        ("devtmpfs", "/dev", "devtmpfs", MountFlags::NOSUID, Some(c"mode=0755")),
        ("tmpfs", "/run", "tmpfs", MountFlags::NOSUID | MountFlags::NODEV, Some(c"mode=0755")),
    ] {
        std::fs::create_dir_all(target).map_err(|error| format!("{target}: {error}"))?;
        mount(source, target, kind, flags, data).map_err(|error| format!("mounting {target}: {error}"))?;
    }
    Ok(())
}

/// Puts the console on standard input, output and error. The kernel only does that if the initramfs carries a `/dev/console` node.
pub fn attach_console() {
    if let Ok(console) = rustix::fs::open("/dev/console", OFlags::RDWR | OFlags::NOCTTY, Mode::empty()) {
        let _ = rustix::stdio::dup2_stdin(&console);
        let _ = rustix::stdio::dup2_stdout(&console);
        let _ = rustix::stdio::dup2_stderr(&console);
    }
}

/// `rootflags=` as the generic mount flags the kernel wants as flags (what `mount -o noatime` does for you) and the rest, which goes
/// to the file system as it is.
pub fn split_flags(list: &str) -> (MountFlags, String) {
    let mut flags = MountFlags::empty();
    let mut rest = Vec::new();
    for option in list.split(',').filter(|option| !option.is_empty()) {
        match option {
            "noatime" => flags |= MountFlags::NOATIME,
            "nodiratime" => flags |= MountFlags::NODIRATIME,
            "relatime" => flags |= MountFlags::RELATIME,
            "nosuid" => flags |= MountFlags::NOSUID,
            "nodev" => flags |= MountFlags::NODEV,
            "noexec" => flags |= MountFlags::NOEXEC,
            "sync" => flags |= MountFlags::SYNCHRONOUS,
            "lazytime" => flags |= MountFlags::LAZYTIME,
            "ro" | "rw" => {}
            other => rest.push(other),
        }
    }
    (flags, rest.join(","))
}

pub fn mount_root(cmdline: &Cmdline) -> Result<(), String> {
    let source = cmdline.root.as_deref().ok_or("there's no root= on the kernel command line")?;
    if !source.starts_with('/') {
        return Err(format!("root={source}: only a device path works here, since there's no udev to look up UUIDs or labels"));
    }
    std::fs::create_dir_all(SYSROOT).map_err(|error| format!("{SYSROOT}: {error}"))?;
    let (generic, rest) = split_flags(cmdline.root_flags.as_deref().unwrap_or(""));
    let mut flags = generic;
    if cmdline.read_only {
        flags |= MountFlags::RDONLY;
    }
    let data = CString::new(rest).map_err(|_| "rootflags has a NUL".to_owned())?;
    let known: Vec<String> = match &cmdline.root_fstype {
        Some(kind) => vec![kind.clone()],
        None => ROOT_TYPES.iter().map(|kind| (*kind).to_owned()).collect(),
    };
    let mut last = String::from("no file system type to try");
    for kind in known {
        let data = if data.as_bytes().is_empty() { None } else { Some(data.as_c_str()) };
        match mount(source, SYSROOT, kind.as_str(), flags, data) {
            Ok(()) => return Ok(()),
            // EINVAL is how the kernel says "this isn't a {kind} file system" (or one it doesn't have): try the next.
            Err(rustix::io::Errno::INVAL) | Err(rustix::io::Errno::NODEV) => last = format!("{source} isn't {kind}"),
            Err(error) => return Err(format!("mounting {source} on {SYSROOT} as {kind}: {error}")),
        }
    }
    Err(last)
}

/// The init to hand over to: `init=` if it's there, else the first of the usual places that exists.
pub fn pick_init(requested: Option<&str>, exists: impl Fn(&str) -> bool) -> Option<String> {
    requested.filter(|path| exists(path)).map(str::to_owned).or_else(|| ["/usr/lib/systemd/systemd", "/lib/systemd/systemd", "/sbin/init", "/usr/sbin/init"].into_iter().find(|path| exists(path)).map(str::to_owned))
}

/// Whether `/` is the initramfs (a ram file system), the only thing that may be emptied.
pub fn root_is_ramfs() -> bool {
    rustix::fs::statfs("/").is_ok_and(|stat| matches!(stat.f_type as u64, RAMFS_MAGIC | TMPFS_MAGIC))
}

/// Deletes what is on the initramfs itself (not other file systems mounted under it), as switch_root does, to give its memory back.
fn empty_initramfs(directory: &Path, root_device: u64) {
    for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.dev() != root_device {
            continue;
        }
        if meta.is_dir() {
            empty_initramfs(&path, root_device);
            let _ = std::fs::remove_dir(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Moves the system over to the unlocked root and runs `init` there as PID 1. Only returns if something went wrong.
pub fn switch_root(init: &str) -> Result<Infallible, String> {
    if root_is_ramfs() {
        if let Ok(meta) = std::fs::metadata("/") {
            empty_initramfs(Path::new("/"), meta.dev());
        }
    }
    for directory in ["proc", "sys", "dev", "run"] {
        let target = format!("{SYSROOT}/{directory}");
        std::fs::create_dir_all(&target).map_err(|error| format!("{target}: {error}"))?;
        mount_move(format!("/{directory}"), &target).map_err(|error| format!("moving /{directory} to {target}: {error}"))?;
    }
    chdir(SYSROOT).map_err(|error| format!("chdir {SYSROOT}: {error}"))?;
    mount_move(".", "/").map_err(|error| format!("moving {SYSROOT} onto /: {error}"))?;
    chroot(".").map_err(|error| format!("chroot: {error}"))?;
    chdir("/").map_err(|error| format!("chdir /: {error}"))?;
    let error = std::process::Command::new(init).exec();
    Err(format!("running {init}: {error}"))
}

pub fn reboot() -> ! {
    rustix::fs::sync();
    let _ = rustix::system::reboot(rustix::system::RebootCommand::Restart);
    // If even that fails, there's nothing left to do but wait; init must never exit.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_options_become_flags_and_the_rest_goes_to_the_file_system() {
        let (flags, rest) = split_flags("noatime,discard,errors=remount-ro,nodev,lazytime");
        assert!(flags.contains(MountFlags::NOATIME | MountFlags::NODEV | MountFlags::LAZYTIME));
        assert_eq!(rest, "discard,errors=remount-ro");
    }

    #[test]
    fn ro_and_rw_in_rootflags_are_left_to_the_command_line() {
        let (flags, rest) = split_flags("rw,ro");
        assert!(flags.is_empty() && rest.is_empty());
    }

    #[test]
    fn empty_rootflags_are_nothing() {
        assert_eq!(split_flags("").1, "");
    }

    #[test]
    fn init_equals_wins_if_it_exists_else_the_usual_places() {
        let has = |paths: &'static [&'static str]| move |path: &str| paths.contains(&path);
        assert_eq!(pick_init(Some("/bin/mine"), has(&["/bin/mine", "/sbin/init"])).as_deref(), Some("/bin/mine"));
        assert_eq!(pick_init(Some("/bin/missing"), has(&["/sbin/init"])).as_deref(), Some("/sbin/init"));
        assert_eq!(pick_init(None, has(&["/usr/lib/systemd/systemd", "/sbin/init"])).as_deref(), Some("/usr/lib/systemd/systemd"));
        assert_eq!(pick_init(None, has(&[])), None);
    }

    #[test]
    fn only_a_ram_file_system_may_be_emptied() {
        // This test runs on a real disk or tmpfs; the point is that the check is the statfs magic, not the path.
        let stat = rustix::fs::statfs("/").unwrap();
        assert_eq!(root_is_ramfs(), matches!(stat.f_type as u64, RAMFS_MAGIC | TMPFS_MAGIC));
    }
}
