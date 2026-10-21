//! `autark-hub-helper <verb> …`: the root half of the Hub. The Hub runs it through `sudo` with the password you type
//! there; it is installed in `/usr/libexec/autark/`. It does only what the verbs below say, checks every name before it
//! runs anything, and runs `autark-pkg` without a shell.
//!
//! ```text
//! install <name>…      remove <name>…      upgrade [<name>…]      rollback <name>
//! ```
//!
//! It never takes a path, a URL or an option, and it always passes `--no-arch`: a package with no recipe is an error,
//! never a download from Arch Linux, so "install" cannot reach anything the recipes folder does not already hold.

use std::process::{Command, Stdio};

const PKG: &str = "/usr/bin/autark-pkg";
const PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const MOST_NAMES: usize = 40;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let pkg_args = match command_line(&args) {
        Ok(pkg_args) => pkg_args,
        Err(error) => {
            eprintln!("autark-hub-helper: {error}");
            std::process::exit(2);
        }
    };
    // Its own output goes straight through, so the Hub can show a build as it happens.
    let status = Command::new(PKG).args(&pkg_args).env_clear().env("PATH", PATH).env("HOME", "/root").stdin(Stdio::null()).status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("autark-hub-helper: couldn't run {PKG}: {error}");
            std::process::exit(1);
        }
    }
}

/// A package name as recipes spell them: letters, digits and `+ . _ -`, not starting with a dash or a dot.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "+._-".contains(c))
}

/// The arguments for `autark-pkg`, or why the request is refused.
fn command_line(args: &[&str]) -> Result<Vec<String>, String> {
    let (verb, names) = args.split_first().ok_or("no verb: install, remove, upgrade or rollback")?;
    if names.len() > MOST_NAMES {
        return Err(format!("at most {MOST_NAMES} packages at a time"));
    }
    if let Some(bad) = names.iter().find(|name| !valid_name(name)) {
        return Err(format!("{bad:?} is not a package name"));
    }
    match (*verb, names.len()) {
        ("install" | "remove", 1..) | ("upgrade", _) | ("rollback", 1) => {}
        ("install" | "remove", 0) => return Err(format!("{verb} needs a package name")),
        ("rollback", _) => return Err("rollback takes one package".into()),
        _ => return Err(format!("unknown verb {verb:?}: install, remove, upgrade or rollback")),
    }
    let mut out = vec!["--no-arch".to_owned(), (*verb).to_owned()];
    out.extend(names.iter().map(|name| (*name).to_owned()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> Result<Vec<String>, String> {
        command_line(args)
    }

    #[test]
    fn the_four_verbs_become_autark_pkg_commands_that_never_import() {
        assert_eq!(line(&["install", "zlib", "libXtst"]).unwrap(), ["--no-arch", "install", "zlib", "libXtst"]);
        assert_eq!(line(&["remove", "g++-libs"]).unwrap(), ["--no-arch", "remove", "g++-libs"]);
        assert_eq!(line(&["upgrade"]).unwrap(), ["--no-arch", "upgrade"]);
        assert_eq!(line(&["upgrade", "curl"]).unwrap(), ["--no-arch", "upgrade", "curl"]);
        assert_eq!(line(&["rollback", "mesa"]).unwrap(), ["--no-arch", "rollback", "mesa"]);
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(line(&[]).is_err());
        assert!(line(&["build", "zlib"]).is_err(), "only the four verbs");
        assert!(line(&["keygen"]).is_err());
        assert!(line(&["install"]).is_err());
        assert!(line(&["remove"]).is_err());
        assert!(line(&["rollback"]).is_err());
        assert!(line(&["rollback", "a", "b"]).is_err());
    }

    #[test]
    fn a_name_cannot_be_an_option_a_path_or_a_shell_string() {
        for bad in ["--force", "-y", "../etc", "/etc/passwd", "a b", "a;b", "$(id)", "a\nb", "", ".hidden", "a|b", "名前", &"x".repeat(65)] {
            assert!(line(&["install", bad]).is_err(), "{bad:?}");
            assert!(line(&["upgrade", bad]).is_err(), "{bad:?}");
        }
        assert!(line(&["install", &"x".repeat(64)]).is_ok());
    }

    #[test]
    fn a_request_for_too_many_packages_is_refused() {
        let many: Vec<String> = (0..41).map(|n| format!("p{n}")).collect();
        let mut args = vec!["upgrade"];
        args.extend(many.iter().map(String::as_str));
        assert!(line(&args).is_err());
    }
}
