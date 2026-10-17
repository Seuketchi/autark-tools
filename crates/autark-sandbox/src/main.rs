//! Sandbox launcher: runs vendor binaries under a per-app Landlock profile.
//!
//! `autark-sandbox run <app> [args…]` reads `/etc/autark/sandbox/<app>.toml`, enters the sandbox and execs
//! the app in its place, so the launcher, the dock and the task manager see the app itself. Desktop files for
//! vendor apps run it as `Exec=autark-sandbox run <app> %U`.

mod profile;
mod sandbox;

use std::os::unix::process::CommandExt;
use std::path::PathBuf;

use profile::Profile;

const PROFILES: &str = "/etc/autark/sandbox";
const USAGE: &str = "usage: autark-sandbox [--profiles <dir>] <command>

  run <app> [args…]    run an app inside its profile's sandbox
  check <app>          show what the profile lets through, and what this kernel can enforce
  list                 the apps with profiles

  --profiles <dir>     profiles folder (default /etc/autark/sandbox)
  --no-namespace       development only: skip the namespaces (Ubuntu's AppArmor blocks them);
                       the session bus and other sockets stay reachable unless Landlock ABI 9 covers them";

fn main() {
    if let Err(error) = run() {
        eprintln!("autark-sandbox: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (mut dir, mut namespace) = (PathBuf::from(PROFILES), true);
    let mut words = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profiles" if words.is_empty() => dir = args.next().ok_or("--profiles needs a folder")?.into(),
            "--no-namespace" if words.is_empty() => namespace = false,
            "-h" | "--help" if words.is_empty() => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => {
                words.push(arg);
                // Everything after the app name belongs to the app.
                if words.len() == 2 && words[0] == "run" {
                    words.extend(args.by_ref());
                }
            }
        }
    }
    match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["run", app, ..] => launch(&Profile::load(&dir, app)?, &words[2..], namespace),
        ["check", app] => check(&Profile::load(&dir, app)?),
        ["list"] => {
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .map_err(|error| format!("{}: {error}", dir.display()))?
                .flatten()
                .filter_map(|entry| entry.file_name().to_string_lossy().strip_suffix(".toml").map(str::to_owned))
                .collect();
            names.sort();
            for name in names {
                let description = Profile::load(&dir, &name).map(|profile| profile.description).unwrap_or_else(|error| format!("(broken: {error})"));
                println!("{name}  {description}");
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn launch(profile: &Profile, extra: &[String], namespace: bool) -> Result<(), String> {
    let plan = sandbox::Plan::new(profile)?;
    if namespace {
        sandbox::namespaces(&plan)?;
    }
    for path in &profile.write {
        if let Ok(path) = profile::expand(path, std::path::Path::new(&std::env::var_os("HOME").unwrap_or_default())) {
            // A writable folder the app expects is made for it, so its first run doesn't fail.
            let _ = std::fs::create_dir_all(path);
        }
    }
    let enforced = sandbox::landlock(&plan)?;
    if !namespace && !enforced.sockets {
        eprintln!("autark-sandbox: warning: without namespaces and Landlock ABI 9, the app can reach the session bus");
    }
    let error = std::process::Command::new(&plan.exec).args(&profile.args).args(extra).env_clear().envs(plan.env.iter().map(|(name, value)| (name, value))).exec();
    Err(format!("{}: {error}", plan.exec.display()))
}

fn check(profile: &Profile) -> Result<(), String> {
    let plan = sandbox::Plan::new(profile)?;
    let abi = landlock_abi();
    println!("{}  {}", plan.exec.display(), profile.description);
    println!(
        "  Landlock ABI {abi} (files{}{}{})",
        if abi >= 4 { ", TCP ports" } else { "" },
        if abi >= 6 { ", signals and abstract sockets" } else { "" },
        if abi >= 9 { ", Unix-socket connects" } else { "" }
    );
    let show = |paths: &[PathBuf]| paths.iter().filter(|path| path.exists()).map(|path| path.display().to_string()).collect::<Vec<_>>().join(" ");
    println!("  read:    {}", show(&plan.read));
    println!("  write:   {}", show(&plan.write));
    println!("  sockets: {}", plan.sockets.iter().map(|(_, name)| name.display().to_string()).collect::<Vec<_>>().join(" "));
    println!("  network: {:?}", plan.net);
    println!("  dbus:    {}", if profile.dbus { "the whole session bus" } else { "none" });
    Ok(())
}

/// The kernel's Landlock ABI version, 0 without Landlock.
fn landlock_abi() -> i64 {
    // SAFETY: landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION) reads nothing and makes nothing.
    let version = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<u8>(), 0usize, 1u32) };
    version.max(0)
}
