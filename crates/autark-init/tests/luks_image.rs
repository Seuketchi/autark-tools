//! The real libcryptsetup, end to end, with no root: a small LUKS2 image file made here, and `autark-init check` run against it
//! the way the initramfs would ask. Nothing is activated (no name is given), so no device-mapper, loop device or privilege is
//! involved. It's the TPM and the mounting that this can't reach; those wait for a qemu boot.

use std::io::Write;
use std::process::{Command, Stdio};

use libcryptsetup_rs::consts::flags::{CryptPbkdf, CryptVolumeKey};
use libcryptsetup_rs::consts::vals::{CryptKdf, EncryptionFormat};
use libcryptsetup_rs::{CryptInit, CryptPbkdfType, Either};

fn image(name: &str, passphrase: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("autark-init-{}-{name}.img", std::process::id()));
    std::fs::File::create(&path).unwrap().set_len(32 * 1024 * 1024).unwrap();
    let mut device = CryptInit::init(&path).unwrap();
    // The cheapest key derivation allowed, so the test takes a moment and not a second of memory-hard hashing.
    let kdf = CryptPbkdfType { type_: CryptKdf::Pbkdf2, hash: Some("sha256".to_owned()), time_ms: 0, iterations: 1000, max_memory_kb: 0, parallel_threads: 0, flags: CryptPbkdf::NO_BENCHMARK };
    device.settings_handle().set_pbkdf_type(&kdf).unwrap();
    device.context_handle().format::<()>(EncryptionFormat::Luks2, ("aes", "xts-plain64"), None, Either::Right(64), None).unwrap();
    device.keyslot_handle().add_by_key(None, None, passphrase.as_bytes(), CryptVolumeKey::empty()).unwrap();
    path
}

/// Runs `autark-init check <image>` typing `typed` (one line each), and gives the exit status and what it printed.
fn check(image: &std::path::Path, typed: &[&str]) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_autark-init")).arg("check").arg(image).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for line in typed {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    (output.status.success(), format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn the_right_passphrase_opens_it() {
    let path = image("right", "correct horse");
    let (ok, said) = check(&path, &["correct horse"]);
    std::fs::remove_file(&path).unwrap();
    assert!(ok, "{said}");
    assert!(said.contains("opens"), "{said}");
}

#[test]
fn a_wrong_one_is_refused_then_the_right_one_works() {
    let path = image("retry", "correct horse");
    let (ok, said) = check(&path, &["wrong", "correct horse"]);
    std::fs::remove_file(&path).unwrap();
    assert!(ok, "{said}");
    assert!(said.contains("That isn't right."), "{said}");
}

#[test]
fn three_wrong_ones_give_up() {
    let path = image("give-up", "correct horse");
    let (ok, said) = check(&path, &["a", "b", "c"]);
    std::fs::remove_file(&path).unwrap();
    assert!(!ok, "{said}");
    assert!(said.contains("three wrong tries"), "{said}");
}

#[test]
fn a_file_that_is_not_luks_is_said_to_be_so() {
    let path = std::env::temp_dir().join(format!("autark-init-{}-plain.img", std::process::id()));
    std::fs::write(&path, vec![0u8; 1024 * 1024]).unwrap();
    let (ok, said) = check(&path, &["x"]);
    std::fs::remove_file(&path).unwrap();
    assert!(!ok);
    assert!(said.contains("isn't a LUKS2 volume"), "{said}");
}

#[test]
fn without_tpm2_the_token_is_not_tried_and_a_passphrase_is_asked_for_straight_away() {
    let path = image("notpm", "pw");
    let (ok, said) = check(&path, &["pw"]);
    std::fs::remove_file(&path).unwrap();
    assert!(ok, "{said}");
    assert!(!said.contains("TPM"), "{said}");
}

#[test]
fn asking_for_the_tpm_on_a_volume_without_one_falls_back_to_the_passphrase() {
    let path = image("tpmless", "pw");
    let mut child = Command::new(env!("CARGO_BIN_EXE_autark-init")).args(["check"]).arg(&path).arg("--tpm2").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "pw").unwrap();
    let output = child.wait_with_output().unwrap();
    std::fs::remove_file(&path).unwrap();
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{said}{}", String::from_utf8_lossy(&output.stderr));
    assert!(said.contains("passphrase or the recovery key instead"), "{said}");
}
