//! libcryptsetup, through its Rust bindings: finding which device holds a LUKS2 volume, and opening it. The TPM part is
//! libcryptsetup's own token plugin (`libcryptsetup-token-systemd-tpm2.so`, which talks to the TPM through tpm2-tss), so no
//! key material and no TPM protocol passes through this program: it hands over a PIN and gets back "opened" or an error.

use std::path::Path;

use libcryptsetup_rs::consts::flags::CryptActivate;
use libcryptsetup_rs::consts::vals::EncryptionFormat;
use libcryptsetup_rs::{CryptDevice, CryptInit, LibcryptErr};

use crate::cmdline::Options;
use crate::unlock::{Attempt, Volume};

const ENOANO: i32 = 55;
/// What the TPM token plugin can say when the TPM itself is the problem: no device, no such token or feature, a timeout.
const NO_TPM: [i32; 6] = [2 /* ENOENT */, 19 /* ENODEV */, 95 /* EOPNOTSUPP */, 110 /* ETIMEDOUT */, 6 /* ENXIO */, 5 /* EIO */];

/// An opened `crypt_device`, ready to be activated by a token or a passphrase.
pub struct Luks {
    device: CryptDevice,
    /// The name under `/dev/mapper`; `None` only checks the secret without activating anything.
    name: Option<String>,
    /// `CryptActivate` bits (the type isn't `Copy`, and each call needs its own).
    flags: u32,
}

/// The UUID of the LUKS2 volume at `path`, lower case, or `None` if it isn't one or can't be read.
pub fn uuid_of(path: &Path) -> Option<String> {
    let mut device = CryptInit::init(path).ok()?;
    device.context_handle().load::<()>(Some(EncryptionFormat::Luks2), None).ok()?;
    device.status_handle().get_uuid().ok().map(|uuid| uuid.to_string().to_ascii_lowercase())
}

pub fn open(path: &Path, name: Option<&str>, options: &Options) -> Result<Luks, String> {
    let mut device = CryptInit::init(path).map_err(|error| format!("{}: {error}", path.display()))?;
    device.context_handle().load::<()>(Some(EncryptionFormat::Luks2), None).map_err(|error| format!("{} isn't a LUKS2 volume: {error}", path.display()))?;
    let mut flags = CryptActivate::empty();
    if options.discard {
        flags |= CryptActivate::ALLOW_DISCARDS;
    }
    if options.no_read_workqueue {
        flags |= CryptActivate::NO_READ_WORKQUEUE;
    }
    if options.no_write_workqueue {
        flags |= CryptActivate::NO_WRITE_WORKQUEUE;
    }
    Ok(Luks { device, name: name.map(str::to_owned), flags: flags.bits() })
}

fn errno(error: &LibcryptErr) -> Option<i32> {
    match error {
        LibcryptErr::IOError(error) => error.raw_os_error(),
        _ => None,
    }
}

impl Volume for Luks {
    fn token(&mut self) -> Attempt {
        match self.device.token_handle().activate_by_token::<()>(self.name.as_deref(), None, None, CryptActivate::from_bits_truncate(self.flags)) {
            Ok(_) => Attempt::Unlocked,
            Err(error) if errno(&error) == Some(ENOANO) => Attempt::NeedsPin,
            Err(error) => Attempt::Unavailable(error.to_string()),
        }
    }

    fn token_with_pin(&mut self, pin: &[u8]) -> Attempt {
        match self.device.token_handle().activate_by_token_pin::<()>(self.name.as_deref(), Some("systemd-tpm2"), None, pin, None, CryptActivate::from_bits_truncate(self.flags)) {
            Ok(_) => Attempt::Unlocked,
            // The TPM isn't there or can't answer: say so and fall back. Anything else counts as a refused PIN, which is the safe
            // way round: it uses one of the three tries, never more.
            Err(error) if errno(&error).is_some_and(|code| NO_TPM.contains(&code)) => Attempt::Unavailable(error.to_string()),
            Err(_) => Attempt::Wrong,
        }
    }

    fn passphrase(&mut self, passphrase: &[u8]) -> Attempt {
        match self.device.activate_handle().activate_by_passphrase(self.name.as_deref(), None, passphrase, CryptActivate::from_bits_truncate(self.flags)) {
            Ok(_) => Attempt::Unlocked,
            // EPERM is a wrong passphrase; a busy or missing device isn't worth retyping for.
            Err(error) => match errno(&error) {
                Some(1) | None => Attempt::Wrong,
                Some(_) => Attempt::Unavailable(error.to_string()),
            },
        }
    }
}
