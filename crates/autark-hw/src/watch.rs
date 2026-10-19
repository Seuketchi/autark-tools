//! `autark-hw watch`: stays running, and when hardware that needs attention appears (a USB device nothing drives, a card
//! whose driver exists but isn't loaded, firmware a driver can't find) it says so in a notification. It looks again once
//! things have settled, so one plugged-in device isn't announced for each of its interfaces.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use autark_hw::doctor::{self, Finding, Machine, Status};
use autark_hw::{aliases, devices, hotplug, kernel_log};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// How long after the last event the machine is looked at again.
const SETTLE: Duration = Duration::from_millis(2500);

fn findings() -> Vec<Finding> {
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|text| text.trim().to_owned()).unwrap_or_default();
    let devices = devices::read();
    let aliases = aliases::load(&kernel);
    let loaded = doctor::loaded_modules();
    let notes = kernel_log::read();
    doctor::diagnose(&Machine { devices: &devices, aliases: &aliases, loaded: &loaded, notes: &notes, firmware_root: Path::new("/lib/firmware") })
}

fn what(finding: &Finding) -> &'static str {
    if !finding.missing_firmware.is_empty() {
        "needs firmware its driver can't find"
    } else {
        match finding.status {
            Status::NoDriver => "has no driver in this kernel",
            Status::Loadable(_) => "has a driver that isn't loaded",
            Status::Declined(_) => "isn't supported by its driver",
            _ => "needs attention",
        }
    }
}

fn notify(summary: &str, body: &str) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return };
    let hints = HashMap::<&str, glib::Variant>::from([("category", "device".to_variant())]);
    let args = ("Hardware", 0u32, "autark-hw", summary, body, Vec::<&str>::new(), hints, -1i32).to_variant();
    let _ = bus.call_sync(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
    );
}

pub fn run() -> ExitCode {
    let mut monitor = match Command::new("udevadm").args(["monitor", "--udev", "--property", "--subsystem-match=usb", "--subsystem-match=pci", "--subsystem-match=thunderbolt", "--subsystem-match=bluetooth"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(monitor) => monitor,
        Err(error) => {
            eprintln!("hardware: can't start udevadm: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(output) = monitor.stdout.take() else { return ExitCode::FAILURE };
    // Lines arrive on their own thread, as events; this one waits for them, and for quiet after the last.
    let (lines, events) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut known: HashSet<String> = hotplug::problem_keys(&findings());
    println!("watching: {} problems known now", known.len());
    let mut chunk = String::new();
    let mut changed_at: Option<Instant> = None;
    loop {
        match events.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                chunk.push_str(&line);
                chunk.push('\n');
                if line.is_empty() {
                    if hotplug::parse_events(&chunk).iter().any(hotplug::worth_a_look) {
                        changed_at = Some(Instant::now());
                    }
                    chunk.clear();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return ExitCode::FAILURE,
        }
        if changed_at.is_some_and(|at| at.elapsed() >= SETTLE) {
            changed_at = None;
            let now = findings();
            for finding in hotplug::newly_wrong(&known, &now) {
                notify(&finding.device.title(), &format!("This {what}. Open Hardware to see what can be done.", what = what(finding)));
            }
            known = hotplug::problem_keys(&now);
        }
    }
}
