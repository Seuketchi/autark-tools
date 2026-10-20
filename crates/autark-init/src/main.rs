//! Initramfs init: TPM-plus-PIN LUKS2 unlock through libcryptsetup (and its systemd TPM2 token), then hands over to systemd.
//!
//! As PID 1 in an initramfs it does the whole boot: mount the virtual file systems, read the kernel command line, find each
//! `rd.luks.name=` volume by its UUID, open it (the TPM and its PIN first when `tpm2-device=` is in `rd.luks.options=`, then a
//! passphrase or recovery key), mount `root=`, and become systemd on it. When anything fails it says what and offers a restart,
//! and the old dracut image stays in the boot menu as the way back in (docs/specs/autark-init.md).
//!
//! Anywhere else (not PID 1) it only does read-only checks, so it can be tried on a running system:
//!
//! ```text
//! autark-init parse [file]              what it makes of a kernel command line (default: /proc/cmdline)
//! autark-init scan                      which block devices it would look inside, and any LUKS2 UUIDs it finds
//! autark-init check <device> [--tpm2]   the real unlock questions against a device or an image, activating nothing
//! ```

mod boot;
mod cmdline;
mod console;
mod cryptsetup;
mod debug;
mod devices;
mod splash;
mod unlock;

use std::convert::Infallible;
use std::time::Duration;

use console::{Console, Terminal};
use splash::console::SplashConsole;
use zeroize::Zeroizing;
use unlock::{Failure, How};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_init = rustix::process::getpid().as_raw_nonzero().get() == 1;
    match args.first().map(String::as_str) {
        Some("parse") if !is_init => parse(args.get(1)),
        Some("scan") if !is_init => scan(),
        Some("check") if !is_init => check(&args[1..]),
        _ if is_init => run_as_init(),
        _ => {
            eprintln!("autark-init is the initramfs's init; run as PID 1 it boots the system.\nTo try it: autark-init parse [file] | scan | check <device> [--tpm2]");
            std::process::exit(2);
        }
    }
}

/// PID 1 must never exit or panic: that's a kernel panic. So every way out of here is a message and a restart.
fn run_as_init() -> ! {
    let outcome = std::panic::catch_unwind(boot).unwrap_or_else(|_| Err("autark-init stopped unexpectedly".to_owned()));
    let Err(error) = outcome;
    let mut console = Terminal;
    console.say(&format!("\nautark-init: {error}"));
    console.wait_for_enter("The old initramfs is in the boot menu as a way back in. Press Enter to restart.");
    boot::reboot()
}

/// The text console, or the loading screen when the command line asks for it and the screen allows it.
enum Screen {
    Text(Terminal),
    Splash(SplashConsole),
}

impl Console for Screen {
    fn say(&mut self, text: &str) {
        match self {
            Screen::Text(console) => console.say(text),
            Screen::Splash(console) => console.say(text),
        }
    }

    fn secret(&mut self, prompt: &str) -> Option<Zeroizing<Vec<u8>>> {
        match self {
            Screen::Text(console) => console.secret(prompt),
            Screen::Splash(console) => console.secret(prompt),
        }
    }

    fn wait_for_enter(&mut self, text: &str) {
        match self {
            Screen::Text(console) => console.wait_for_enter(text),
            Screen::Splash(console) => console.wait_for_enter(text),
        }
    }

    fn unlocked(&mut self) {
        if let Screen::Splash(console) = self {
            Console::unlocked(console);
        }
    }
}

fn boot() -> Result<Infallible, String> {
    boot::mount_special()?;
    boot::attach_console();
    let line = std::fs::read_to_string("/proc/cmdline").map_err(|error| format!("/proc/cmdline: {error}"))?;
    let cmdline = cmdline::parse(&line);
    if let Some(device) = &cmdline.log_device {
        if let Err(error) = debug::start_file(device) {
            eprintln!("autark-init: no boot log on {device}: {error}");
        }
    }
    debug::log(&format!("starting: splash {}, {} encrypted volume(s)", if cmdline.splash { "requested" } else { "off" }, cmdline.luks.len()));
    // A splash that can't draw is the text prompt, not a failure.
    let mut console = match cmdline.splash.then(SplashConsole::start) {
        Some(Ok(splash)) => Screen::Splash(splash),
        Some(Err(why)) => {
            debug::log(&format!("no splash, using the text prompt: {why}"));
            Screen::Text(Terminal)
        }
        None => Screen::Text(Terminal),
    };
    if cmdline.luks.is_empty() {
        console.say("No rd.luks.name= on the command line: not unlocking anything.");
    }
    for volume in &cmdline.luks {
        if cmdline.debug && !volume.options.ignored.is_empty() {
            console.say(&format!("Ignoring options for {}: {}", volume.name, volume.options.ignored.join(", ")));
        }
        console.say(&format!("Looking for the encrypted disk {}…", volume.uuid));
        let path = devices::find(&volume.uuid, Duration::from_secs(cmdline.timeout)).ok_or_else(|| format!("the encrypted disk {} didn't appear within {} seconds", volume.uuid, cmdline.timeout))?;
        let mut luks = cryptsetup::open(&path, Some(&volume.name), &volume.options)?;
        match unlock::unlock(&mut luks, &mut console, volume.options.tpm2) {
            Ok(How::Tpm) => console.say(&format!("{} unlocked with the TPM.", volume.name)),
            Ok(How::Passphrase) => console.say(&format!("{} unlocked.", volume.name)),
            Err(failure) => return Err(describe(&failure, &volume.name)),
        }
        debug::log(&format!("{} is unlocked", volume.name));
        console.unlocked();
    }
    boot::mount_root(&cmdline)?;
    debug::log("root is mounted; handing over to the system");
    debug::stop_file();
    let init = boot::pick_init(cmdline.init.as_deref(), |path| std::path::Path::new(boot::SYSROOT).join(path.trim_start_matches('/')).exists()).ok_or("there's no init (systemd) on the root file system")?;
    boot::switch_root(&init)
}

fn describe(failure: &Failure, name: &str) -> String {
    match failure {
        Failure::NoAnswer => format!("nothing was typed to unlock {name}"),
        Failure::TooManyTries => format!("{name} wasn't unlocked: three wrong tries"),
        Failure::Device(why) => format!("{name} can't be opened: {why}"),
    }
}

// ---- checks that are safe to run anywhere

fn parse(file: Option<&String>) {
    let line = match file {
        Some(file) => std::fs::read_to_string(file).unwrap_or_else(|error| fail(&format!("{file}: {error}"))),
        None => std::fs::read_to_string("/proc/cmdline").unwrap_or_else(|error| fail(&format!("/proc/cmdline: {error}"))),
    };
    println!("{:#?}", cmdline::parse(&line));
}

fn scan() {
    for name in devices::candidates(std::path::Path::new("/sys/class/block")) {
        let path = std::path::Path::new("/dev").join(&name);
        match cryptsetup::uuid_of(&path) {
            Some(uuid) => println!("{name}: LUKS2 {uuid}"),
            None => println!("{name}"),
        }
    }
}

fn check(args: &[String]) {
    let Some(device) = args.first() else { fail("check needs a device or an image") };
    let tpm2 = args.iter().any(|arg| arg == "--tpm2");
    let options = cmdline::Options { tpm2, ..cmdline::Options::default() };
    // No name: libcryptsetup only checks the secret and activates nothing.
    let mut luks = cryptsetup::open(std::path::Path::new(device), None, &options).unwrap_or_else(|error| fail(&error));
    let mut console = Terminal;
    match unlock::unlock(&mut luks, &mut console, tpm2) {
        Ok(How::Tpm) => println!("The TPM would unlock {device}."),
        Ok(How::Passphrase) => println!("That opens {device}."),
        Err(failure) => fail(&describe(&failure, device)),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("autark-init: {message}");
    std::process::exit(1);
}
