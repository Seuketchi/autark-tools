//! Putting it together: for each device, whether a driver runs it, and if not why, with what the kernel said about it.
//! A pure function of what was read, so it's checked with made-up machines.

use std::path::Path;

use crate::aliases::{self, Alias};
use crate::devices::{Class, Device};
use crate::kernel_log::{self, Kind, Note};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// A driver runs it.
    Working,
    /// The kernel has a driver for it that isn't loaded.
    Loadable(Vec<String>),
    /// A driver was loaded for it and didn't take it.
    Declined(Vec<String>),
    /// No driver in this kernel for it, and it's the kind that needs one.
    NoDriver,
    /// No driver, and it doesn't need one.
    Quiet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub device: Device,
    pub status: Status,
    /// Firmware its driver asked for that isn't installed.
    pub missing_firmware: Vec<String>,
    /// What its driver said about this model, if it doesn't know it.
    pub unsupported: Vec<String>,
}

impl Finding {
    /// Whether it's something to look at.
    pub fn is_problem(&self) -> bool {
        !matches!(self.status, Status::Working | Status::Quiet) || !self.missing_firmware.is_empty() || !self.unsupported.is_empty()
    }
}

fn same(a: &str, b: &str) -> bool {
    a.replace('-', "_") == b.replace('-', "_")
}

/// What the machine has to say for itself: its devices, what the kernel notes, what's loaded.
pub struct Machine<'a> {
    pub devices: &'a [Device],
    pub aliases: &'a [Alias],
    pub loaded: &'a [String],
    pub notes: &'a [Note],
    pub firmware_root: &'a Path,
}

pub fn diagnose(machine: &Machine) -> Vec<Finding> {
    machine
        .devices
        .iter()
        .map(|device| {
            let mut candidates: Vec<String> = Vec::new();
            for alias in &device.modaliases {
                for module in aliases::modules_for(machine.aliases, alias) {
                    if !candidates.contains(&module) {
                        candidates.push(module);
                    }
                }
            }
            // A "usb" or "usbhid"-style generic driver doesn't count as the answer for the device itself.
            let driver_names: Vec<&String> = device.drivers.iter().filter(|driver| driver.as_str() != "usb").collect();
            let status = if !driver_names.is_empty() {
                Status::Working
            } else if candidates.is_empty() {
                if device.class.needs_driver() { Status::NoDriver } else { Status::Quiet }
            } else if candidates.iter().any(|module| machine.loaded.iter().any(|loaded| same(loaded, module))) {
                Status::Declined(candidates.clone())
            } else if device.class.needs_driver() {
                Status::Loadable(candidates.clone())
            } else {
                Status::Quiet
            };
            // The notes a device's drivers, bound or merely candidates, made.
            let mut speakers: Vec<&str> = driver_names.iter().map(|driver| driver.as_str()).collect();
            speakers.extend(candidates.iter().map(String::as_str));
            let heard: Vec<&Note> = machine.notes.iter().filter(|note| speakers.iter().any(|speaker| same(speaker, &note.driver))).collect();
            let mut missing_firmware = Vec::new();
            let mut unsupported = Vec::new();
            for note in heard {
                match &note.kind {
                    Kind::MissingFirmware(name) if !kernel_log::installed(name, machine.firmware_root) => missing_firmware.push(name.clone()),
                    Kind::Unsupported => unsupported.push(note.text.clone()),
                    Kind::MissingFirmware(_) => {}
                }
            }
            Finding { device: device.clone(), status, missing_firmware, unsupported }
        })
        .collect()
}

/// The "not supported" complaints of drivers that run none of the listed devices: a laptop's embedded controller, say,
/// which isn't on the PCI or USB buses at all.
pub fn loose_notes<'a>(findings: &[Finding], notes: &'a [Note]) -> Vec<&'a Note> {
    notes.iter().filter(|note| note.kind == Kind::Unsupported && !findings.iter().any(|finding| finding.device.drivers.iter().any(|driver| same(driver, &note.driver)))).collect()
}

/// Devices that look like lighting controllers, which no standard interface covers.
pub fn lighting_devices(findings: &[Finding]) -> Vec<&Device> {
    findings
        .iter()
        .map(|finding| &finding.device)
        .filter(|device| {
            let name = format!("{} {}", device.vendor, device.model).to_lowercase();
            ["mystic", "rgb", "lighting", "steelseries", "per-key"].iter().any(|word| name.contains(word))
        })
        .collect()
}

/// The module names in `/proc/modules`.
pub fn loaded_modules() -> Vec<String> {
    std::fs::read_to_string("/proc/modules").map(|text| text.lines().filter_map(|line| line.split_whitespace().next().map(str::to_owned)).collect()).unwrap_or_default()
}

/// What to do about a finding, in a sentence; `None` when nothing needs doing.
pub fn advice(finding: &Finding) -> Option<String> {
    let device = &finding.device;
    if !finding.missing_firmware.is_empty() {
        return Some(format!("Its driver wants {} and it isn't installed: add it to the firmware package.", finding.missing_firmware.join(", ")));
    }
    match &finding.status {
        Status::Loadable(modules) => Some(format!("This kernel has a driver for it ({}) that isn't loaded: load it, or let the hardware service do it.", modules.join(", "))),
        Status::Declined(modules) => Some(format!("{} loaded and didn't take it, so this model isn't supported by that driver version.", modules.join(", "))),
        Status::NoDriver if device.class == Class::Graphics && device.id.starts_with("10de:") => {
            Some("This kernel has no driver for this NVIDIA GPU: it needs the open nouveau driver (with its GSP firmware) or NVIDIA's own.".to_owned())
        }
        Status::NoDriver => Some(format!("This kernel has no driver for {} ({}): it needs a kernel built with one.", device.class.label().to_lowercase(), device.id)),
        _ if !finding.unsupported.is_empty() => Some("The kernel's driver for this laptop says it doesn't support this model's firmware version.".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Bus;

    fn device(id: &str, class: Class, drivers: &[&str], aliases: &[&str]) -> Device {
        Device {
            path: String::new(),
            bus: Bus::Pci,
            address: format!("slot-{id}"),
            id: id.to_owned(),
            vendor: String::new(),
            model: String::new(),
            class,
            drivers: drivers.iter().map(|driver| (*driver).to_owned()).collect(),
            modaliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        }
    }

    fn machine_findings(devices: &[Device], loaded: &[&str], log: &str) -> Vec<Finding> {
        let aliases = aliases::parse("alias pci:v10DEd25A2* nouveau\nalias pci:v1002d* amdgpu\nalias pci:v8086d51F1* iwlwifi\nalias pci:v8086d7777* gizmo\n");
        let loaded: Vec<String> = loaded.iter().map(|name| (*name).to_owned()).collect();
        let notes = kernel_log::parse(log);
        diagnose(&Machine { devices, aliases: &aliases, loaded: &loaded, notes: &notes, firmware_root: Path::new("/nonexistent-firmware-root") })
    }

    #[test]
    fn each_kind_of_situation_is_told_apart() {
        let devices = [
            device("8086:51f1", Class::Wireless, &["iwlwifi"], &["pci:v8086d51F1"]),
            device("10de:25a2", Class::Graphics, &[], &["pci:v10DEd25A2"]),
            device("1002:73ff", Class::Graphics, &[], &["pci:v1002d73FF"]),
            device("8086:7777", Class::Network, &[], &["pci:v8086d7777"]),
            device("9999:0001", Class::Network, &[], &["pci:v9999d0001"]),
            device("8086:519d", Class::Bridge, &[], &["pci:v8086d519D"]),
        ];
        let findings = machine_findings(&devices, &["amdgpu"], "");
        let statuses: Vec<&Status> = findings.iter().map(|finding| &finding.status).collect();
        assert_eq!(statuses[0], &Status::Working);
        assert_eq!(statuses[1], &Status::Loadable(vec!["nouveau".into()]));
        assert_eq!(statuses[2], &Status::Declined(vec!["amdgpu".into()]));
        assert_eq!(statuses[3], &Status::Loadable(vec!["gizmo".into()]));
        assert_eq!(statuses[4], &Status::NoDriver);
        assert_eq!(statuses[5], &Status::Quiet);
        assert!(findings[0].is_problem() == false && findings[4].is_problem() && !findings[5].is_problem());
    }

    #[test]
    fn what_the_kernel_said_attaches_to_the_device_whose_driver_said_it() {
        let devices = [device("8086:51f1", Class::Wireless, &["iwlwifi"], &["pci:v8086d51F1"]), device("1462:1601", Class::Input, &["usbhid"], &[])];
        let log = "\
Oct 19 10:19:13 autark kernel: iwlwifi 0000:00:14.3: Direct firmware load for iwlwifi-so-a0-gf-a0-99.ucode failed with error -2
Oct 19 10:19:12 autark kernel: msi_ec: Firmware version is not supported: '15K1IMS1.113'
";
        let findings = machine_findings(&devices, &[], log);
        assert_eq!(findings[0].missing_firmware, ["iwlwifi-so-a0-gf-a0-99.ucode"]);
        assert!(findings[0].is_problem());
        assert!(advice(&findings[0]).unwrap().contains("iwlwifi-so-a0-gf-a0-99.ucode"));
        // The laptop driver's complaint belongs to no PCI or USB device; it's reported on its own.
        assert!(findings[1].unsupported.is_empty());
    }

    #[test]
    fn complaints_from_drivers_with_no_listed_device_are_loose() {
        let devices = [device("8086:51f1", Class::Wireless, &["iwlwifi"], &[])];
        let log = "\
Oct 19 10:19:12 autark kernel: msi_ec: Firmware version is not supported: '15K1IMS1.113'
Oct 19 10:19:12 autark kernel: iwlwifi 0000:00:14.3: something is not supported here
";
        let findings = machine_findings(&devices, &[], log);
        let notes = kernel_log::parse(log);
        let loose = loose_notes(&findings, &notes);
        assert_eq!(loose.len(), 1);
        assert_eq!(loose[0].driver, "msi_ec");
    }

    #[test]
    fn lighting_controllers_are_spotted_by_name() {
        let mut lights = device("1462:1601", Class::Input, &["usbhid"], &[]);
        lights.model = "MysticLight MS-1565".into();
        let plain = device("046d:c52b", Class::Input, &["usbhid"], &[]);
        let findings = machine_findings(&[lights, plain], &[], "");
        let found = lighting_devices(&findings);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "1462:1601");
    }

    #[test]
    fn advice_fits_the_case() {
        let nvidia = machine_findings(&[device("10de:2204", Class::Graphics, &[], &[])], &[], "").remove(0);
        assert!(advice(&nvidia).unwrap().contains("nouveau"));
        let other = machine_findings(&[device("9999:0001", Class::Audio, &[], &[])], &[], "").remove(0);
        assert!(advice(&other).unwrap().contains("no driver for audio"));
        let fine = machine_findings(&[device("8086:51f1", Class::Wireless, &["iwlwifi"], &[])], &[], "").remove(0);
        assert_eq!(advice(&fine), None);
    }
}
