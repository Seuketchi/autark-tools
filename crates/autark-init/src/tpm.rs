//! The "OS separator". systemd's first step in an initramfs is to extend TPM PCRs 0 to 7 with the SHA-256 of the string
//! `os-separator` (`systemd-pcrextend --early-os-separator`, "TPM PCR OS Separator" in the journal). A key sealed from a running
//! Autark is therefore sealed to PCR 7 *after* that extend, and the firmware's own value (what is there when this init starts)
//! can't unseal it: the TPM says "policy does not match". So this init does the same extend before it asks the TPM for anything.
//! The TPM is spoken to directly, one raw `TPM2_PCR_Extend` command per PCR on `/dev/tpmrm0`; libcryptsetup's TPM token only unseals.

use std::fs::OpenOptions;
use std::io::{Read, Write};

/// SHA-256 of `os-separator`, which is the value systemd extends.
const DIGEST: [u8; 32] = [
    0xff, 0x5b, 0x9d, 0x73, 0xda, 0xd7, 0x09, 0x63, 0x3a, 0xe7, 0x6a, 0xdf, 0x44, 0x40, 0x12, 0xb5, 0x7e, 0x91, 0x3a, 0x12, 0xed, 0x74, 0x03, 0xc3, 0x93, 0x11, 0x45, 0x86, 0x2f, 0x35, 0xf8, 0x41,
];
const PCRS: std::ops::RangeInclusive<u32> = 0..=7;
const DEVICES: [&str; 2] = ["/dev/tpmrm0", "/dev/tpm0"];

/// `TPM2_PCR_Extend` of the SHA-256 bank, authorised by the empty password session (PCRs 0 to 7 have no auth value).
fn command(pcr: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(65);
    bytes.extend_from_slice(&0x8002u16.to_be_bytes()); // TPM_ST_SESSIONS
    bytes.extend_from_slice(&65u32.to_be_bytes()); // the whole command's size
    bytes.extend_from_slice(&0x182u32.to_be_bytes()); // TPM_CC_PCR_Extend
    bytes.extend_from_slice(&pcr.to_be_bytes()); // a PCR's handle is its number
    bytes.extend_from_slice(&9u32.to_be_bytes()); // size of the authorisation area
    bytes.extend_from_slice(&0x4000_0009u32.to_be_bytes()); // TPM_RS_PW
    bytes.extend_from_slice(&0u16.to_be_bytes()); // no nonce
    bytes.push(0); // no session attributes
    bytes.extend_from_slice(&0u16.to_be_bytes()); // empty password
    bytes.extend_from_slice(&1u32.to_be_bytes()); // one digest
    bytes.extend_from_slice(&0x000bu16.to_be_bytes()); // TPM_ALG_SHA256
    bytes.extend_from_slice(&DIGEST);
    bytes
}

/// The response code in a TPM response (zero is success).
fn response_code(response: &[u8]) -> Result<u32, String> {
    if response.len() < 10 {
        return Err(format!("the TPM's answer was {} bytes, too short", response.len()));
    }
    Ok(u32::from_be_bytes([response[6], response[7], response[8], response[9]]))
}

fn extend(device: &mut (impl Read + Write), pcr: u32) -> Result<(), String> {
    device.write_all(&command(pcr)).map_err(|error| format!("writing to the TPM: {error}"))?;
    let mut response = [0u8; 4096];
    let length = device.read(&mut response).map_err(|error| format!("reading from the TPM: {error}"))?;
    match response_code(&response[..length])? {
        0 => Ok(()),
        code => Err(format!("the TPM refused the extend of PCR {pcr}: response code {code:#x}")),
    }
}

/// Extends PCRs 0 to 7 with the OS separator. Failing is not fatal to the boot (the passphrase still opens the disk), so the
/// caller logs the reason and goes on.
pub fn extend_os_separator() -> Result<(), String> {
    let mut last = String::from("no TPM device");
    for path in DEVICES {
        let mut device = match OpenOptions::new().read(true).write(true).open(path) {
            Ok(device) => device,
            Err(error) => {
                last = format!("{path}: {error}");
                continue;
            }
        };
        for pcr in PCRS {
            extend(&mut device, pcr).map_err(|error| format!("{path}: {error}"))?;
        }
        return Ok(());
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TPM that answers each command with a canned response and keeps what it was sent.
    struct Fake {
        code: u32,
        sent: Vec<Vec<u8>>,
    }

    impl Write for Fake {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.sent.push(bytes.to_vec());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Read for Fake {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let mut response = vec![0x80, 0x01, 0, 0, 0, 10];
            response.extend_from_slice(&self.code.to_be_bytes());
            buffer[..10].copy_from_slice(&response);
            Ok(10)
        }
    }

    #[test]
    fn the_command_is_a_well_formed_pcr_extend() {
        let bytes = command(7);
        assert_eq!(bytes.len(), 65);
        assert_eq!(u32::from_be_bytes(bytes[2..6].try_into().unwrap()) as usize, bytes.len(), "the size field says how long it is");
        assert_eq!(bytes[..2], [0x80, 0x02]);
        assert_eq!(bytes[6..10], [0, 0, 0x01, 0x82]);
        assert_eq!(bytes[10..14], [0, 0, 0, 7], "PCR 7");
        assert_eq!(bytes[14..18], [0, 0, 0, 9], "an authorisation area of nine bytes");
        assert_eq!(bytes[18..22], [0x40, 0, 0, 0x09], "the password session");
        assert_eq!(bytes[27..33], [0, 0, 0, 1, 0, 0x0b], "one digest, SHA-256");
        assert_eq!(bytes[33..], DIGEST);
    }

    #[test]
    fn it_extends_every_pcr_from_0_to_7_once() {
        let mut tpm = Fake { code: 0, sent: Vec::new() };
        for pcr in PCRS {
            extend(&mut tpm, pcr).unwrap();
        }
        let handles: Vec<u8> = tpm.sent.iter().map(|sent| sent[13]).collect();
        assert_eq!(handles, [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn a_refusal_is_reported_with_its_code() {
        let mut tpm = Fake { code: 0x98e, sent: Vec::new() };
        assert!(extend(&mut tpm, 7).unwrap_err().contains("0x98e"));
    }

    #[test]
    fn a_short_answer_is_an_error() {
        assert!(response_code(&[0x80, 0x01]).is_err());
    }
}
