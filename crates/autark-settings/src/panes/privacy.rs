//! Privacy & Security: what protects this laptop, and the switches that are yours to turn. The firewall, encrypted DNS
//! and name broadcasts are root's, so they go through the helper with your password (see `privileged.rs`); the
//! assistant's switches are `ai.toml`; the microphone is PipeWire's. Disk encryption and Secure Boot are chosen when the
//! system is installed, so they're shown and explained, not offered as switches.

use std::cell::Cell;
use std::process::Command;
use std::rc::Rc;

use gtk::prelude::*;
use serde_json::Value;

use super::network::{dns_mode, resolved_config};
use crate::{config, privileged, ui};

const TLS: [(&str, &str); 3] = [("yes", "Required"), ("opportunistic", "When the server can"), ("no", "Off")];

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Privacy & Security", Some("What protects this laptop, and what you can turn on or off.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);

    let device = ui::group();
    let firewall = guarded_switch(active("nftables"), "turn the firewall on or off", &status, |on| vec!["service", if on { "enable" } else { "disable" }, "nftables"], || active("nftables"));
    device.append(&ui::row("Firewall", Some("Blocks connections nobody asked for (nftables)"), &firewall));
    device.append(&ui::row("Disk encryption", Some("Chosen when the system was installed; it locks the disk when the laptop is off"), &ui::value(disk_encryption())));
    device.append(&ui::row("Secure Boot", Some("Chosen in the firmware settings; only signed boot software may start"), &ui::value(secure_boot())));
    column.append(&ui::section("This laptop", None, &device));

    let config_now = resolved_config();
    let network = ui::group();
    let tls = config_now.get("DNSOverTLS").map(String::as_str).unwrap_or("no");
    network.append(&ui::row(
        "Encrypted DNS",
        Some("Lookups can't be read or changed on the way"),
        &ui::choice(&TLS, tls, {
            let status = status.clone();
            move |choice| {
                if choice == "yes" && dns_mode().0 == "network" {
                    status.set_label("Required encryption needs servers with a name; the network's DNS doesn't have one. Pick a provider on the Network pane first.");
                    status.set_visible(true);
                    return;
                }
                privileged::apply(&status.clone(), "change whether DNS is encrypted", &["resolved-tls", choice], None, &status, |_| {});
            }
        }),
    ));
    let announcing = config_now.get("LLMNR").is_none_or(|value| value != "no") || config_now.get("MulticastDNS").is_none_or(|value| value != "no");
    let names = guarded_switch(announcing, "change whether this laptop announces its name", &status, |on| vec!["resolved-multicast", if on { "yes" } else { "no" }], move || {
        let now = resolved_config();
        now.get("LLMNR").is_none_or(|value| value != "no") || now.get("MulticastDNS").is_none_or(|value| value != "no")
    });
    network.append(&ui::row("Announce this laptop's name", Some("LLMNR and mDNS tell everyone on the Wi-Fi what it's called, and can be spoofed. Off is safer."), &names));
    column.append(&ui::section("Network", None, &network));

    let ai = config::read("ai");
    let assistant = ui::group();
    let set = {
        let status = status.clone();
        Rc::new(move |key: &'static str, on: bool| match config::edit("ai", |document| config::set_in(document, key, on)) {
            Ok(()) => status.set_visible(false),
            Err(error) => {
                status.set_label(&error);
                status.set_visible(true);
            }
        })
    };
    assistant.append(&ui::row(
        "Kith",
        Some("The assistant. Off, nothing it does runs."),
        &ui::switch(config::get_bool(&ai, "enabled").unwrap_or(false), {
            let set = set.clone();
            move |on| set("enabled", on)
        }),
    ));
    assistant.append(&ui::row(
        "Keep everything on this laptop",
        Some("Nothing is ever sent to a server; features that need one stay off"),
        &ui::switch(config::get_bool(&ai, "local_only").unwrap_or(false), {
            let set = set.clone();
            move |on| set("local_only", on)
        }),
    ));
    column.append(&ui::section("Assistant", None, &assistant));

    let devices = ui::group();
    let muted = microphone_muted();
    let mic = ui::switch(muted.unwrap_or(false), {
        let status = status.clone();
        move |on| {
            let result = Command::new("wpctl").args(["set-mute", "@DEFAULT_AUDIO_SOURCE@", if on { "1" } else { "0" }]).status();
            let failed = !result.is_ok_and(|result| result.success());
            status.set_visible(failed);
            if failed {
                status.set_label("Couldn't reach the sound system (PipeWire).");
            }
        }
    });
    mic.set_sensitive(muted.is_some());
    devices.append(&ui::row("Mute the microphone", Some("Every app hears silence until you turn this off"), &mic));
    column.append(&ui::section("Microphone", None, &devices));

    let more = ui::group();
    more.append(&ui::row(
        "Lock screen and fingerprints",
        Some("When the screen locks, what it shows, and how you unlock it"),
        &ui::button("Open…", |button| {
            let _ = button.activate_action("win.show-pane", Some(&"lock".to_variant()));
        }),
    ));
    column.append(&ui::section("More", None, &more));
    column.append(&status);
    page.upcast()
}

/// A switch that runs the helper with `args(on)` when flipped, and shows what is true afterwards (`now`), whatever
/// happened, so it never claims a change that didn't go through.
fn guarded_switch(on: bool, reason: &'static str, status: &gtk::Label, args: impl Fn(bool) -> Vec<&'static str> + 'static, now: impl Fn() -> bool + 'static) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(on);
    switch.set_valign(gtk::Align::Center);
    let (status, working, now, args) = (status.clone(), Rc::new(Cell::new(false)), Rc::new(now), Rc::new(args));
    switch.connect_state_set(move |switch, wanted| {
        if working.get() {
            return gtk::glib::Propagation::Stop;
        }
        working.set(true);
        let (switch, status, working, now) = (switch.clone(), status.clone(), working.clone(), now.clone());
        privileged::apply(&switch.clone(), reason, &args(wanted), None, &status, |_| {});
        // Settle on the truth a moment later, once the helper (or the cancelled dialog) is done.
        gtk::glib::timeout_add_seconds_local_once(2, move || {
            let truth = now();
            switch.set_state(truth);
            switch.set_active(truth);
            working.set(false);
        });
        gtk::glib::Propagation::Stop
    });
    switch
}

fn text(command: &[&str]) -> String {
    Command::new(command[0]).args(&command[1..]).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
}

fn active(unit: &str) -> bool {
    text(&["systemctl", "is-active", unit]).trim() == "active"
}

/// `Some(true)` when the default microphone is muted; `None` when PipeWire can't say.
fn microphone_muted() -> Option<bool> {
    let output = Command::new("wpctl").args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"]).output().ok().filter(|output| output.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).contains("[MUTED]"))
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
