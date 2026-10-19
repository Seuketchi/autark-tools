//! The program the qemu test puts on the encrypted test disk as the "real init". If `autark-init` hands over properly it runs
//! as PID 1 on the unlocked root, and says so on the console for the test driver to read, then powers off. It isn't part of a
//! real boot; it's installed nowhere. Modes, from `autark.test=` on the kernel command line:
//!
//! - `check` (default): checks the handover, prints `TEST ok: …` or `TEST FAIL: …` for each thing, then `AUTARK-INIT-TEST PASS` or `FAIL`.
//! - `enroll`: enrols the TPM2 + PIN and a recovery key on the disk with systemd-cryptenroll (`autark.test.device=`,
//!   `autark.test.passphrase=`, `autark.test.pin=`), and prints the recovery key as `TEST recovery-key: …`.

use std::process::Command;

use rustix::system::{RebootCommand, reboot};

/// One mount from `/proc/self/mountinfo`.
#[derive(Debug, PartialEq, Eq)]
struct Mount {
    point: String,
    fstype: String,
    source: String,
    options: String,
}

fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let fields: Vec<&str> = before.split_whitespace().collect();
            let rest: Vec<&str> = after.split_whitespace().collect();
            Some(Mount { point: (*fields.get(4)?).to_owned(), options: (*fields.get(5)?).to_owned(), fstype: (*rest.first()?).to_owned(), source: (*rest.get(1)?).to_owned() })
        })
        .collect()
}

fn value(cmdline: &str, key: &str) -> Option<String> {
    cmdline.split_whitespace().find_map(|word| word.strip_prefix(&format!("{key}=")).map(str::to_owned))
}

fn check(cmdline: &str) -> bool {
    let mounts = parse_mountinfo(&std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default());
    let find = |point: &str| mounts.iter().rev().find(|mount| mount.point == point);
    let root = value(cmdline, "root").unwrap_or_default();
    let mut results: Vec<(String, bool)> = vec![
        ("running as PID 1".into(), std::process::id() == 1),
        (format!("/ is {root}"), find("/").is_some_and(|mount| mount.source == root)),
        ("/ is read-write as the command line says".into(), find("/").is_some_and(|mount| mount.options.split(',').any(|option| option == "rw")) == cmdline.split_whitespace().any(|word| word == "rw")),
        ("/proc is proc".into(), find("/proc").is_some_and(|mount| mount.fstype == "proc")),
        ("/sys is sysfs".into(), find("/sys").is_some_and(|mount| mount.fstype == "sysfs")),
        ("/dev is devtmpfs".into(), find("/dev").is_some_and(|mount| mount.fstype == "devtmpfs")),
        ("/run is tmpfs".into(), find("/run").is_some_and(|mount| mount.fstype == "tmpfs")),
        ("the initramfs is gone (no /init, no /sysroot)".into(), !std::path::Path::new("/init").exists() && !std::path::Path::new("/sysroot").exists()),
        ("the test marker is on the root".into(), std::path::Path::new("/test-payload").exists()),
    ];
    if cmdline.contains("tpm2-device") {
        results.push(("the TPM is visible to the real init".into(), std::path::Path::new("/dev/tpmrm0").exists()));
    }
    let mut all = true;
    for (what, fine) in &results {
        println!("TEST {}: {what}", if *fine { "ok" } else { "FAIL" });
        all &= fine;
    }
    all
}

fn enroll(cmdline: &str) -> bool {
    let (Some(device), Some(passphrase), Some(pin)) = (value(cmdline, "autark.test.device"), value(cmdline, "autark.test.passphrase"), value(cmdline, "autark.test.pin")) else {
        println!("TEST FAIL: enroll needs autark.test.device, .passphrase and .pin");
        return false;
    };
    let run = |args: &[&str], envs: &[(&str, &str)]| -> Option<String> {
        let output = Command::new("/usr/bin/systemd-cryptenroll").args(args).envs(envs.iter().copied()).output().ok()?;
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        println!("TEST output: {}", text.trim().replace('\n', " | "));
        output.status.success().then_some(text)
    };
    let tpm = run(&["--tpm2-device=/dev/tpmrm0", "--tpm2-pcrs=7", "--tpm2-with-pin=yes", &device], &[("PASSWORD", &passphrase), ("NEWPIN", &pin)]);
    println!("TEST {}: enrolled the TPM2 + PIN", if tpm.is_some() { "ok" } else { "FAIL" });
    let recovery = run(&["--recovery-key", &device], &[("PASSWORD", &passphrase)]);
    // The key is printed as eight blocks of eight letters and digits, with dashes.
    let key = recovery.as_deref().and_then(|text| text.split_whitespace().find(|word| word.len() == 71 && word.matches('-').count() == 7));
    match key {
        Some(key) => println!("TEST recovery-key: {key}"),
        None => println!("TEST FAIL: no recovery key in the output"),
    }
    tpm.is_some() && key.is_some()
}

fn main() {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let passed = match value(&cmdline, "autark.test").as_deref() {
        Some("enroll") => enroll(&cmdline),
        _ => check(&cmdline),
    };
    println!("AUTARK-INIT-TEST {}", if passed { "PASS" } else { "FAIL" });
    rustix::fs::sync();
    let _ = reboot(RebootCommand::PowerOff);
    // PID 1 can't exit; if powering off failed, wait to be killed.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "36 35 98:0 / / rw,relatime shared:1 - ext4 /dev/mapper/autark rw\n\
        22 36 0:21 / /proc rw,nosuid,nodev,noexec shared:12 - proc proc rw\n\
        23 36 0:22 / /dev rw,nosuid shared:2 - devtmpfs devtmpfs rw,mode=755\n\
        24 36 0:23 / /run rw,nosuid,nodev shared:3 - tmpfs tmpfs rw,mode=755\n";

    #[test]
    fn mountinfo_is_read_by_field_not_by_guess() {
        let mounts = parse_mountinfo(INFO);
        assert_eq!(mounts.len(), 4);
        assert_eq!(mounts[0], Mount { point: "/".into(), fstype: "ext4".into(), source: "/dev/mapper/autark".into(), options: "rw,relatime".into() });
        assert_eq!(mounts[1].fstype, "proc");
        assert_eq!(mounts[2].point, "/dev");
    }

    #[test]
    fn a_line_that_is_not_mountinfo_is_skipped() {
        assert!(parse_mountinfo("not a mount line\n\n").is_empty());
    }

    #[test]
    fn command_line_values_are_found_by_exact_key() {
        let line = "root=/dev/mapper/autark autark.test=enroll autark.test.pin=2468";
        assert_eq!(value(line, "root").as_deref(), Some("/dev/mapper/autark"));
        assert_eq!(value(line, "autark.test").as_deref(), Some("enroll"));
        assert_eq!(value(line, "autark.test.pin").as_deref(), Some("2468"));
        assert_eq!(value(line, "autark"), None);
    }
}
