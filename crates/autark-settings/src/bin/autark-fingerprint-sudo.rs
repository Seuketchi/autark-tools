//! `autark-fingerprint-sudo on|off`: turns the fingerprint for sudo on or off by writing
//! `/etc/pam.d/autark-fingerprint`, which Autark's `/etc/pam.d/sudo` includes. It writes nothing else.
//! Settings runs it through `sudo`, with the password you type there; it's installed in `/usr/libexec/autark/`.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const FILE: &str = "/etc/pam.d/autark-fingerprint";

fn main() {
    let on = match std::env::args().nth(1).as_deref() {
        Some("on") => true,
        Some("off") => false,
        _ => {
            eprintln!("usage: autark-fingerprint-sudo on|off");
            std::process::exit(2);
        }
    };
    if let Err(error) = write(Path::new(FILE), on) {
        eprintln!("autark-fingerprint-sudo: {FILE}: {error}");
        std::process::exit(1);
    }
}

/// The file's text: a fingerprint is enough, and the password still works when it isn't given.
fn text(on: bool) -> &'static str {
    if on {
        "# Written by Settings (autark-fingerprint-sudo): a fingerprint is enough for sudo.\nauth sufficient pam_fprintd.so\n"
    } else {
        "# Written by Settings (autark-fingerprint-sudo): sudo asks for the password only.\n"
    }
}

/// Replaces the file whole, so PAM never reads half of it.
fn write(path: &Path, on: bool) -> std::io::Result<()> {
    let temporary = path.with_extension("new");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(text(on).as_bytes())?;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_include_file() {
        let path = std::env::temp_dir().join(format!("autark-fingerprint-{}", std::process::id()));
        write(&path, true).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().lines().any(|line| line == "auth sufficient pam_fprintd.so"));
        write(&path, false).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("pam_fprintd"));
        std::fs::remove_file(&path).unwrap();
    }
}
