//! What the kernel said about hardware while starting: firmware that couldn't be loaded, and drivers that found their
//! hardware but don't know that model. Read through `journalctl -k`, so there's no need to be root.

use std::path::Path;
use std::process::Command;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Kind {
    /// A driver asked for this firmware file and it isn't there.
    MissingFirmware(String),
    /// A driver found its kind of hardware but doesn't support this model or version.
    Unsupported,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Note {
    /// The driver that said it (`i915`, `msi_ec`).
    pub driver: String,
    pub kind: Kind,
    /// The line, without its date and host.
    pub text: String,
}

/// The driver and message of a kernel line: `Oct 19 10:19:12 autark kernel: msi_ec: Firmware ...`.
fn after_kernel(line: &str) -> Option<&str> {
    line.split_once(" kernel: ").map(|(_, rest)| rest)
}

/// The words before the first colon, without a device address: `iwlwifi 0000:00:14.3: ...` is `iwlwifi`.
fn driver_of(message: &str) -> String {
    message.split_once(':').map_or(message, |(first, _)| first).split_whitespace().next().unwrap_or("").to_owned()
}

pub fn parse(log: &str) -> Vec<Note> {
    let mut notes: Vec<Note> = Vec::new();
    for line in log.lines() {
        let Some(message) = after_kernel(line) else { continue };
        let kind = if let Some(rest) = message.split("Direct firmware load for ").nth(1) {
            rest.split_whitespace().next().map(|name| Kind::MissingFirmware(name.to_owned()))
        } else if let Some(rest) = message.split("firmware: failed to load ").nth(1) {
            rest.split(|c: char| c.is_whitespace() || c == '(').next().map(|name| Kind::MissingFirmware(name.to_owned()))
        } else if message.to_lowercase().contains("is not supported") && !message.contains("[Firmware Bug]") {
            Some(Kind::Unsupported)
        } else {
            None
        };
        if let Some(kind) = kind {
            let note = Note { driver: driver_of(message), kind, text: message.to_owned() };
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
    }
    notes
}

/// The notes from this boot's kernel log; none if the log can't be read.
pub fn read() -> Vec<Note> {
    let output = Command::new("journalctl").args(["-k", "-b", "--no-pager", "-q"]).output();
    output.ok().filter(|output| output.status.success()).map(|output| parse(&String::from_utf8_lossy(&output.stdout))).unwrap_or_default()
}

/// Whether the firmware `name` is installed, as the kernel looks for it, plain or compressed.
pub fn installed(name: &str, root: &Path) -> bool {
    ["", ".zst", ".xz"].iter().any(|ext| root.join(format!("{name}{ext}")).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
Oct 19 10:19:12 autark kernel: wmi_bus wmi_bus-PNP0C14:00: [Firmware Bug]: WQAK data block query control method not found
Oct 19 10:19:12 autark kernel: iwlwifi 0000:00:14.3: loaded firmware version 89.d2579d43.0 so-a0-gf-a0-89.ucode op_mode iwlmvm
Oct 19 10:19:12 autark kernel: msi_ec: Firmware version is not supported: '15K1IMS1.113'
Oct 19 10:19:13 autark kernel: ath11k_pci 0000:03:00.0: Direct firmware load for ath11k/QCA6390/hw2.0/board-2.bin failed with error -2
Oct 19 10:19:13 autark kernel: ath11k_pci 0000:03:00.0: Direct firmware load for ath11k/QCA6390/hw2.0/board-2.bin failed with error -2
Oct 19 10:19:14 autark kernel: rtw89_pci 0000:04:00.0: firmware: failed to load rtw89/rtw8852a_fw.bin (-2)
Oct 19 10:19:14 autark systemd[1]: unrelated unit message about firmware
";

    #[test]
    fn missing_firmware_and_unsupported_models_are_noted_once() {
        let notes = parse(LOG);
        assert_eq!(notes.len(), 3, "{notes:#?}");
        assert_eq!(notes[0], Note { driver: "msi_ec".into(), kind: Kind::Unsupported, text: "msi_ec: Firmware version is not supported: '15K1IMS1.113'".into() });
        assert_eq!((notes[1].driver.as_str(), &notes[1].kind), ("ath11k_pci", &Kind::MissingFirmware("ath11k/QCA6390/hw2.0/board-2.bin".into())));
        assert_eq!((notes[2].driver.as_str(), &notes[2].kind), ("rtw89_pci", &Kind::MissingFirmware("rtw89/rtw8852a_fw.bin".into())));
    }

    #[test]
    fn firmware_is_found_plain_or_compressed() {
        let dir = std::env::temp_dir().join(format!("autark-hw-fw-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("intel")).unwrap();
        std::fs::write(dir.join("intel/a.bin.zst"), "").unwrap();
        std::fs::write(dir.join("b.bin"), "").unwrap();
        assert!(installed("intel/a.bin", &dir) && installed("b.bin", &dir) && !installed("c.bin", &dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
