//! Noticing new hardware as it arrives. `udevadm monitor` reports every device that appears or goes; after things settle
//! the machine is looked at again, and a problem that wasn't there before (a USB device with no driver, a card the
//! kernel has a driver for but hasn't loaded) is worth telling the person about.

use std::collections::HashSet;

use crate::doctor::Finding;

/// One event from `udevadm monitor --udev --property`.
#[derive(Debug, PartialEq, Eq)]
pub struct Event {
    pub action: String,
    pub subsystem: String,
}

/// The events in a chunk of `udevadm monitor --property` output: a header line like
/// `UDEV  [123.4] add      /devices/... (usb)`, then `KEY=value` lines, then a blank line.
pub fn parse_events(text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    for line in text.lines() {
        if !line.starts_with("UDEV") && !line.starts_with("KERNEL") {
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(_origin) = words.next() else { continue };
        // A real event has its time in brackets next; the monitor's own introduction ("UDEV - the event which…") doesn't.
        if !words.next().is_some_and(|time| time.starts_with('[')) {
            continue;
        }
        let Some(action) = words.next() else { continue };
        let subsystem = line.rsplit_once('(').and_then(|(_, rest)| rest.strip_suffix(')')).unwrap_or("").to_owned();
        events.push(Event { action: action.to_owned(), subsystem });
    }
    events
}

/// Whether an event could mean new hardware worth a second look: something added or removed on the buses that carry it.
pub fn worth_a_look(event: &Event) -> bool {
    matches!(event.action.as_str(), "add" | "remove") && matches!(event.subsystem.as_str(), "usb" | "pci" | "thunderbolt" | "bluetooth" | "drm")
}

/// What identifies a problem, so the same one isn't announced twice: the device, and what's wrong.
pub fn key(finding: &Finding) -> String {
    format!("{}|{}|{:?}|{}", finding.device.address, finding.device.id, finding.status, finding.missing_firmware.join(","))
}

/// The problems in `now` that weren't in `before`.
pub fn newly_wrong<'a>(before: &HashSet<String>, now: &'a [Finding]) -> Vec<&'a Finding> {
    now.iter().filter(|finding| finding.is_problem() && !before.contains(&key(finding))).collect()
}

/// The keys of every current problem, to compare the next look against.
pub fn problem_keys(now: &[Finding]) -> HashSet<String> {
    now.iter().filter(|finding| finding.is_problem()).map(key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{Bus, Class, Device};
    use crate::doctor::Status;

    #[test]
    fn events_are_read_from_the_monitor_stream() {
        let text = "\
monitor will print the received events for:
UDEV - the event which udev sends out after rule processing

UDEV  [1234.567] add      /devices/pci0000:00/0000:00:14.0/usb1/1-3 (usb)
ACTION=add
DEVTYPE=usb_device

UDEV  [1234.600] bind     /devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3:1.0 (usb)
UDEV  [1240.001] remove   /devices/virtual/net/tun0 (net)
";
        let events = parse_events(text);
        assert_eq!(events.len(), 3, "{events:?}");
        assert_eq!(events[0], Event { action: "add".into(), subsystem: "usb".into() });
        assert!(worth_a_look(&events[0]));
        assert!(!worth_a_look(&events[1]), "a bind isn't new hardware");
        assert!(!worth_a_look(&events[2]), "a network interface isn't a device to look at");
    }

    fn finding(address: &str, status: Status, firmware: &[&str]) -> Finding {
        Finding {
            device: Device {
                path: String::new(),
                bus: Bus::Usb,
                address: address.into(),
                id: "1234:5678".into(),
                vendor: String::new(),
                model: String::new(),
                class: Class::Network,
                drivers: Vec::new(),
                modaliases: Vec::new(),
            },
            status,
            missing_firmware: firmware.iter().map(|name| (*name).to_owned()).collect(),
            unsupported: Vec::new(),
        }
    }

    #[test]
    fn only_a_problem_that_is_new_is_announced() {
        let before_findings = [finding("1-1", Status::NoDriver, &[]), finding("1-2", Status::Working, &[])];
        let before = problem_keys(&before_findings);
        assert_eq!(before.len(), 1);
        let now = [finding("1-1", Status::NoDriver, &[]), finding("1-2", Status::Working, &[]), finding("1-3", Status::NoDriver, &[]), finding("1-4", Status::Working, &["x.bin"])];
        let new = newly_wrong(&before, &now);
        let addresses: Vec<&str> = new.iter().map(|finding| finding.device.address.as_str()).collect();
        assert_eq!(addresses, ["1-3", "1-4"]);
        // Looked at again with nothing changed, nothing is new.
        assert!(newly_wrong(&problem_keys(&now), &now).is_empty());
    }
}
