//! Entering the sandbox, in this process, just before exec'ing the app:
//!
//! 1. An unprivileged user and mount namespace (and a network one for `network = "none"`), with empty
//!    folders over `/run/user/<uid>` and `/tmp` and only the Wayland and PipeWire sockets put back. This hides
//!    the session bus, where systemd could start anything outside the sandbox.
//! 2. Landlock: files (read/execute and write lists), TCP ports, signals and abstract sockets scoped to the
//!    sandbox, and on kernels with Landlock ABI 9 (Linux 7.1) Unix-socket connects too.
//! 3. No new privileges, so a setuid binary can't lift any of it.

use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use landlock::{ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope};
use rustix::fs::{Mode, OFlags};
use rustix::mount::{MountFlags, MountPropagationFlags};
use rustix::thread::UnshareFlags;

use crate::profile::{Net, Profile, expand};

/// Read and execute, for every app: the system itself, never anyone's files.
const SYSTEM: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/opt/share", "/proc", "/sys", "/dev", "/run/systemd/resolve"];
/// Read and write, for every app.
const DEVICES: &[&str] = &["/dev/null", "/dev/zero", "/dev/full", "/dev/random", "/dev/urandom", "/dev/tty", "/dev/shm"];
/// The look of the desktop, read-only from home: fonts, cursors and toolkit settings.
const LOOK: &[&str] = &["~/.local/share/fonts", "~/.fonts", "~/.icons", "~/.config/fontconfig", "~/.config/gtk-3.0", "~/.config/gtk-4.0", "~/.cache/fontconfig"];
/// Environment kept from the caller; everything else is dropped.
const KEEP_ENV: &[&str] = &["HOME", "USER", "LOGNAME", "LANG", "LANGUAGE", "TZ", "XDG_RUNTIME_DIR", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "XCURSOR_THEME", "XCURSOR_SIZE", "TERM"];

/// Everything the sandbox lets through, resolved for this user.
#[derive(Debug)]
pub struct Plan {
    pub exec: PathBuf,
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
    /// Sockets put back into the empty runtime folder: (real path, name in the folder).
    pub sockets: Vec<(PathBuf, PathBuf)>,
    pub net: Net,
    pub runtime: PathBuf,
    pub env: Vec<(OsString, OsString)>,
}

impl Plan {
    pub fn new(profile: &Profile) -> Result<Self, String> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME isn't set")?);
        let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR isn't set")?);
        let mut read: Vec<PathBuf> = SYSTEM.iter().map(PathBuf::from).collect();
        read.push(profile.exec.parent().unwrap_or(Path::new("/")).to_path_buf());
        for path in LOOK.iter().copied().chain(profile.read.iter().map(String::as_str)) {
            read.push(expand(path, &home)?);
        }
        let mut write: Vec<PathBuf> = DEVICES.iter().map(PathBuf::from).collect();
        write.extend([PathBuf::from("/tmp"), runtime.clone()]);
        if profile.gpu {
            write.push("/dev/dri".into());
        }
        for path in &profile.write {
            let path = expand(path, &home)?;
            if path == home {
                return Err("write = [\"~\"] would hand over all of home; name the folders".into());
            }
            write.push(path);
        }

        let mut sockets = Vec::new();
        let mut env: Vec<(OsString, OsString)> = KEEP_ENV.iter().filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value))).collect();
        env.extend(std::env::vars_os().filter(|(name, _)| name.to_string_lossy().starts_with("LC_")));
        env.push(("PATH".into(), "/usr/bin:/bin".into()));
        if profile.wayland {
            let display = std::env::var_os("WAYLAND_DISPLAY").ok_or("the profile wants Wayland, but WAYLAND_DISPLAY isn't set")?;
            let real = runtime.join(&display);
            let name = PathBuf::from(real.file_name().ok_or("WAYLAND_DISPLAY names no socket")?);
            env.push(("WAYLAND_DISPLAY".into(), name.clone().into()));
            sockets.push((real, name));
        }
        if profile.audio {
            sockets.push((runtime.join("pipewire-0"), "pipewire-0".into()));
            sockets.push((runtime.join("pulse/native"), "pulse/native".into()));
        }
        if profile.dbus {
            sockets.push((runtime.join("bus"), "bus".into()));
            env.push(("DBUS_SESSION_BUS_ADDRESS".into(), format!("unix:path={}", runtime.join("bus").display()).into()));
        }
        sockets.retain(|(real, _)| real.exists());
        for (name, value) in &profile.env {
            env.push((name.into(), value.into()));
        }
        Ok(Self { exec: profile.exec.clone(), read, write, sockets, net: profile.net()?, runtime, env })
    }
}

/// Step 1. `Err` when the kernel or a security module won't allow unprivileged namespaces.
pub fn namespaces(plan: &Plan) -> Result<(), String> {
    // Opened before anything covers them; bound back in by their descriptors.
    let mut kept: Vec<(OwnedFd, &Path)> = Vec::new();
    for (real, name) in &plan.sockets {
        let fd = rustix::fs::open(real, OFlags::PATH | OFlags::CLOEXEC, Mode::empty()).map_err(|error| format!("{}: {error}", real.display()))?;
        kept.push((fd, name));
    }
    let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());
    let mut flags = UnshareFlags::NEWUSER | UnshareFlags::NEWNS;
    if plan.net == Net::None {
        flags |= UnshareFlags::NEWNET;
    }
    // SAFETY: called while the process has one thread, and FILES isn't unshared.
    unsafe { rustix::thread::unshare_unsafe(flags) }.map_err(|error| format!("can't make a user namespace ({error}); on Ubuntu, AppArmor's restrict_unprivileged_userns blocks it"))?;
    let map = |file: &str, contents: String| {
        std::fs::write(format!("/proc/self/{file}"), contents)
            .map_err(|error| format!("can't set up the user namespace ({file}: {error}); on Ubuntu, AppArmor's restrict_unprivileged_userns blocks it"))
    };
    map("setgroups", "deny".into())?;
    map("uid_map", format!("{uid} {uid} 1"))?;
    map("gid_map", format!("{gid} {gid} 1"))?;
    rustix::mount::mount_change("/", MountPropagationFlags::REC | MountPropagationFlags::PRIVATE).map_err(|error| format!("making mounts private: {error}"))?;

    let tmpfs = |target: &Path| rustix::mount::mount("tmpfs", target, "tmpfs", MountFlags::NOSUID | MountFlags::NODEV, c"mode=700").map_err(|error| format!("{}: {error}", target.display()));
    tmpfs(&plan.runtime)?;
    tmpfs(Path::new("/tmp"))?;
    for (fd, name) in &kept {
        let target = plan.runtime.join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(&target, "").map_err(|error| format!("{}: {error}", target.display()))?;
        rustix::mount::mount_bind(format!("/proc/self/fd/{}", fd.as_raw_fd()), &target).map_err(|error| format!("{}: {error}", target.display()))?;
    }
    Ok(())
}

/// What Landlock enforced.
pub struct Enforced {
    /// Unix-socket connects (Landlock ABI 9).
    pub sockets: bool,
}

/// Steps 2 and 3. Refuses when Landlock isn't on, or when the profile limits TCP ports and the kernel can't.
pub fn landlock(plan: &Plan) -> Result<Enforced, String> {
    let abi = ABI::V9;
    let error = |error: landlock::RulesetError| format!("Landlock: {error}");
    let mut ruleset = Ruleset::default().handle_access(AccessFs::from_all(abi)).map_err(error)?;
    // "none" is the network namespace's job; refusing TCP too covers running without one.
    if plan.net == Net::None {
        ruleset = ruleset.handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp).map_err(error)?;
    }
    if let Net::Ports { .. } = plan.net {
        ruleset = ruleset
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)
            .map_err(|_| "the profile limits TCP ports, which needs Landlock ABI 4 (Linux 6.7)".to_owned())?
            .set_compatibility(CompatLevel::BestEffort);
    }
    ruleset = ruleset.scope(Scope::from_all(abi)).map_err(error)?;
    let mut created = ruleset.create().map_err(error)?;

    let read = AccessFs::from_read(abi);
    let write = AccessFs::from_all(abi) & !AccessFs::ResolveUnix;
    for (paths, access) in [(&plan.read, read), (&plan.write, write)] {
        for path in paths {
            // A missing path has nothing to allow; the app gets an error if it looks.
            let Ok(fd) = PathFd::new(path) else { continue };
            created = created.add_rule(PathBeneath::new(fd, access)).map_err(error)?;
        }
    }
    for (_, name) in &plan.sockets {
        let Ok(fd) = PathFd::new(plan.runtime.join(name)) else { continue };
        created = created.add_rule(PathBeneath::new(fd, AccessFs::ResolveUnix | AccessFs::ReadFile | AccessFs::WriteFile)).map_err(error)?;
    }
    if let Net::Ports { connect, bind } = &plan.net {
        for port in connect {
            created = created.add_rule(NetPort::new(*port, AccessNet::ConnectTcp)).map_err(error)?;
        }
        for port in bind {
            created = created.add_rule(NetPort::new(*port, AccessNet::BindTcp)).map_err(error)?;
        }
    }
    let status = created.no_new_privs(true).restrict_self().map_err(error)?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err("Landlock isn't enabled in this kernel, so the app would run unconfined".into());
    }
    let abi = match status.landlock {
        landlock::LandlockStatus::Available { effective_abi, kernel_abi } => Some(kernel_abi.unwrap_or(effective_abi as i32)),
        _ => None,
    };
    Ok(Enforced { sockets: abi.is_some_and(|abi| abi >= 9) })
}
