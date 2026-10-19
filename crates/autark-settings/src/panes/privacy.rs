//! Privacy & Security: what protects this laptop, read from the system (never written here): whether the disk is
//! encrypted, Secure Boot, the firewall, and what the network leaks about it. Nothing here asks for permission to be
//! changed, because changing any of it is root's work and is done in the build scripts.

use std::process::Command;

use gtk::prelude::*;
use serde_json::Value;

use crate::{config, ui};

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Privacy & Security", Some("What protects this laptop right now.")));

    let device = ui::group();
    device.append(&ui::row("Disk encryption", Some("Whether this system's disk is locked when the laptop is off"), &ui::value(disk_encryption())));
    device.append(&ui::row("Secure Boot", Some("Only signed boot software may start"), &ui::value(secure_boot())));
    device.append(&ui::row("Firewall", Some("nftables"), &ui::value(if active("nftables") { "On" } else { "Off" })));
    column.append(&ui::section("This laptop", None, &device));

    let network = ui::group();
    let status = text(&["resolvectl", "status"]);
    network.append(&ui::row("Encrypted DNS", Some("Lookups can't be read on the way"), &ui::value(if status.contains("+DNSOverTLS") { "On" } else { "Off" })));
    network.append(&ui::row("Name broadcasts", Some("LLMNR and mDNS tell the local network what this laptop is called"), &ui::value(if status.contains("+LLMNR") || status.contains("+mDNS") { "On" } else { "Off" })));
    column.append(&ui::section("Network", None, &network));

    let ai = config::read("ai");
    let local = config::get_bool(&ai, "local_only").unwrap_or(false);
    let enabled = config::get_bool(&ai, "enabled").unwrap_or(false);
    let kith = ui::group();
    let summary = match (enabled, local) {
        (false, _) => "Kith is off",
        (true, true) => "On, and nothing leaves the laptop",
        (true, false) => "On; some features may send text off the laptop",
    };
    kith.append(&ui::row("Kith", Some("Change this on the AI pane"), &ui::value(summary)));
    column.append(&ui::section("Assistant", None, &kith));
    column.append(&ui::note("Fingerprint, lock screen and password settings are on Fingerprint & Lock."));
    page.upcast()
}

fn text(command: &[&str]) -> String {
    Command::new(command[0]).args(&command[1..]).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
}

fn active(unit: &str) -> bool {
    text(&["systemctl", "is-active", unit]).trim() == "active"
}

/// Whether any block device in `lsblk`'s tree is a LUKS mapping.
fn disk_encryption() -> &'static str {
    let tree: Value = serde_json::from_str(&text(&["lsblk", "-J", "-o", "NAME,TYPE,MOUNTPOINTS"])).unwrap_or(Value::Null);
    fn crypt(devices: &Value, root: bool) -> bool {
        devices.as_array().is_some_and(|all| {
            all.iter().any(|device| {
                let mounted_root = device["mountpoints"].as_array().is_some_and(|points| points.iter().any(|point| point == "/"));
                (device["type"] == "crypt" && (mounted_root || !root)) || crypt(&device["children"], root)
            })
        })
    }
    if crypt(&tree["blockdevices"], true) {
        "On"
    } else if crypt(&tree["blockdevices"], false) {
        "On for some disks, not this one"
    } else {
        "Off"
    }
}

fn secure_boot() -> &'static str {
    match std::fs::read("/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c") {
        Ok(bytes) if bytes.last() == Some(&1) => "On",
        Ok(_) => "Off",
        Err(_) if !std::path::Path::new("/sys/firmware/efi").exists() => "Not available (this isn't an EFI boot)",
        Err(_) => "Unknown",
    }
}
