//! `autark-settings-helper <verb> …`: the root half of Settings. Settings runs it through `sudo` with the password you type
//! there; it's installed in `/usr/libexec/autark/`. It does only what the verbs below say, checks every argument before
//! it touches anything, and runs other programs without a shell. A new password comes on standard input, after the line
//! sudo reads for itself.
//!
//! ```text
//! resolved-dns default | network | custom <server>…   which DNS servers (a server is an address, or address#name for TLS)
//! resolved-tls yes | opportunistic | no                encrypted DNS
//! resolved-multicast yes | no                          LLMNR and mDNS, together
//! service enable | disable bluetooth | nftables        start now and at boot, or stop now and at boot
//! timezone <zone>      ntp on | off      locale <LANG>
//! user-add <name> yes | no                             the second yes or no is administrator; password on stdin
//! user-password <name>                                 new password on stdin
//! user-fullname <name> <text>    user-admin <name> yes | no    user-shell <name> <shell>
//! user-remove <name> keep-home | remove-home
//! pkg-check            pkg-upgrade
//! ```

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RESOLVED_DROPIN: &str = "/etc/systemd/resolved.conf.d/zz-autark-settings.conf";
const NETWORK_DIR: &str = "/etc/systemd/network";
const NETWORK_DROPIN: &str = "zz-autark-dns.conf";
const PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const SERVICES: [&str; 2] = ["bluetooth", "nftables"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // sudo has set this; without it the "me" checks below can't know who to protect.
    let caller = std::env::var("SUDO_USER").unwrap_or_default();
    match run(&args, &caller) {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
        }
        Err(error) => {
            eprintln!("autark-settings-helper: {error}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[&str], caller: &str) -> Result<String, String> {
    match args {
        ["resolved-dns", "default"] => resolved_dns(&Dns::Default),
        ["resolved-dns", "network"] => resolved_dns(&Dns::Network),
        ["resolved-dns", "custom", servers @ ..] => {
            let servers = servers.iter().map(|server| valid_server(server)).collect::<Result<Vec<_>, _>>()?;
            if servers.is_empty() || servers.len() > 6 {
                return Err("give one to six DNS servers".into());
            }
            resolved_dns(&Dns::Custom(servers))
        }
        ["resolved-tls", mode @ ("yes" | "opportunistic" | "no")] => resolved_set(&[("DNSOverTLS", Some((*mode).to_owned()))]),
        ["resolved-multicast", mode @ ("yes" | "no")] => resolved_set(&[("LLMNR", Some((*mode).to_owned())), ("MulticastDNS", Some((*mode).to_owned()))]),
        ["service", action @ ("enable" | "disable"), unit] if SERVICES.contains(unit) => service(action, unit),
        ["timezone", zone] => {
            check_zone(zone)?;
            command("timedatectl", &["set-timezone", zone])
        }
        ["ntp", mode @ ("on" | "off")] => command("timedatectl", &["set-ntp", if *mode == "on" { "true" } else { "false" }]),
        ["locale", locale] => {
            check_locale(locale)?;
            command("localectl", &["set-locale", &format!("LANG={locale}")])
        }
        ["user-add", name, admin @ ("yes" | "no")] => user_add(name, *admin == "yes"),
        ["user-password", name] => user_password(name),
        ["user-fullname", name, text] => {
            person(name, caller, false)?;
            valid_full_name(text)?;
            command("usermod", &["-c", text, name])
        }
        ["user-admin", name, admin @ ("yes" | "no")] => user_admin(name, *admin == "yes", caller),
        ["user-shell", name, shell] => {
            person(name, caller, false)?;
            check_shell(shell)?;
            command("usermod", &["-s", shell, name])
        }
        ["user-remove", name, home @ ("keep-home" | "remove-home")] => {
            person(name, caller, true)?;
            if administrators().iter().all(|admin| admin == name) {
                return Err("that's the only administrator; make someone else one first".into());
            }
            command("userdel", &if *home == "remove-home" { vec!["-r", *name] } else { vec![*name] })
        }
        ["pkg-check"] => {
            check_recipes()?;
            command("autark-pkg", &["-n", "upgrade"])
        }
        ["pkg-upgrade"] => {
            check_recipes()?;
            command("autark-pkg", &["upgrade"])
        }
        _ => Err(format!("not something it does: {}", args.join(" "))),
    }
}

// ---- checks (each one is tested)

/// A DNS server: an IPv4 or IPv6 address, then optionally `#` and the name its certificate carries.
fn valid_server(server: &str) -> Result<String, String> {
    let (address, name) = match server.split_once('#') {
        Some((address, name)) => (address, Some(name)),
        None => (server, None),
    };
    address.parse::<IpAddr>().map_err(|_| format!("{server:?} isn't an IP address"))?;
    if let Some(name) = name {
        let fine = !name.is_empty() && name.len() <= 253 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if !fine {
            return Err(format!("{name:?} isn't a host name"));
        }
    }
    Ok(server.to_owned())
}

/// A login name as useradd allows them, kept strict.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// The "full name" field of an account: no separators the file uses, nothing unprintable.
fn valid_full_name(text: &str) -> Result<(), String> {
    let fine = text.len() <= 60 && !text.chars().any(|c| c.is_control() || matches!(c, ':' | ',' | '=' | '\\' | '"'));
    if fine { Ok(()) } else { Err("a name can't have colons, commas, equals signs, quotes or backslashes, and no more than 60 characters".into()) }
}

fn valid_password(password: &str) -> Result<(), String> {
    if password.is_empty() || password.contains(['\n', '\r', '\0']) {
        Err("that password can't be used".into())
    } else {
        Ok(())
    }
}

fn check_zone(zone: &str) -> Result<(), String> {
    let fine = !zone.is_empty() && !zone.starts_with('/') && !zone.contains("..") && zone.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'));
    if fine && Path::new("/usr/share/zoneinfo").join(zone).is_file() { Ok(()) } else { Err(format!("{zone:?} isn't a time zone")) }
}

fn check_locale(locale: &str) -> Result<(), String> {
    let fine = !locale.is_empty() && locale.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '@'));
    let known = command("locale", &["-a"]).unwrap_or_default().lines().any(|line| line.replace(".utf8", ".UTF-8") == locale || line == locale);
    if fine && known { Ok(()) } else { Err(format!("{locale:?} isn't a locale this system has")) }
}

fn check_shell(shell: &str) -> Result<(), String> {
    let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default().lines().any(|line| line == shell);
    if listed && Path::new(shell).is_file() { Ok(()) } else { Err(format!("{shell:?} isn't a shell this system lists")) }
}

/// `autark-pkg` runs the recipes' scripts as root, so they must be root's and nobody else's to write: a recipes folder
/// (or a file or parent in it) that a user could change would hand that user root.
fn check_recipes() -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    const RECIPES: &str = "/usr/src/autark/scripts/pkg";
    let mut path = Path::new(RECIPES).to_path_buf();
    let meta = std::fs::metadata(&path).map_err(|_| format!("the package recipes aren't installed ({RECIPES})"))?;
    if !meta.is_dir() {
        return Err(format!("{RECIPES} isn't a folder"));
    }
    loop {
        let meta = std::fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(format!("{} isn't owned by root or can be written by others, so Settings won't run its scripts as root", path.display()));
        }
        if !path.pop() || path == Path::new("/") {
            return Ok(());
        }
    }
}

// ---- accounts

struct Passwd {
    name: String,
    uid: u32,
}

fn passwd() -> Vec<Passwd> {
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            Some(Passwd { name: (*fields.first()?).to_owned(), uid: fields.get(2)?.parse().ok()? })
        })
        .collect()
}

/// The members of the administrators' group.
fn administrators() -> Vec<String> {
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let line = group.lines().find(|line| line.starts_with("wheel:")).unwrap_or("");
    line.split(':').nth(3).unwrap_or("").split(',').filter(|name| !name.is_empty()).map(str::to_owned).collect()
}

/// Whether `name` is a person's account Settings may change: it exists, isn't root or a system account, and (when
/// `not_me`) isn't the account asking.
fn person(name: &str, caller: &str, not_me: bool) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("{name:?} isn't a user name"));
    }
    match passwd().iter().find(|account| account.name == name) {
        Some(account) if account.uid >= 1000 && account.uid < 65000 => {}
        Some(_) => return Err(format!("{name} is a system account")),
        None => return Err(format!("there's no account called {name}")),
    }
    if not_me && name == caller {
        return Err("you can't remove the account you're logged in as".into());
    }
    Ok(())
}

fn user_add(name: &str, admin: bool) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!("{name:?} isn't a good user name: lower-case letters, digits, - and _, starting with a letter"));
    }
    if passwd().iter().any(|account| account.name == name) {
        return Err(format!("there's already an account called {name}"));
    }
    let password = read_password()?;
    let mut args = vec!["-m", "-s", "/bin/bash"];
    if admin {
        args.extend(["-G", "wheel"]);
    }
    args.push(name);
    command("useradd", &args)?;
    set_password(name, &password)?;
    Ok(format!("added {name}"))
}

fn user_password(name: &str) -> Result<String, String> {
    person(name, "", false)?;
    let password = read_password()?;
    set_password(name, &password)?;
    Ok(format!("changed the password for {name}"))
}

fn user_admin(name: &str, admin: bool, caller: &str) -> Result<String, String> {
    person(name, caller, false)?;
    let admins = administrators();
    if admin {
        return command("usermod", &["-aG", "wheel", name]);
    }
    if name == caller {
        return Err("you can't take administrator rights from the account you're logged in as".into());
    }
    if admins.iter().all(|member| member == name) {
        return Err("that's the only administrator".into());
    }
    command("gpasswd", &["-d", name, "wheel"])
}

fn read_password() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|error| error.to_string())?;
    let password = line.trim_end_matches(['\n', '\r']).to_owned();
    valid_password(&password)?;
    Ok(password)
}

fn set_password(name: &str, password: &str) -> Result<(), String> {
    let mut child = Command::new("chpasswd").env_clear().env("PATH", PATH).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|error| format!("chpasswd: {error}"))?;
    child.stdin.take().ok_or("chpasswd: no input")?.write_all(format!("{name}:{password}\n").as_bytes()).map_err(|error| error.to_string())?;
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    if output.status.success() { Ok(()) } else { Err(format!("chpasswd: {}", String::from_utf8_lossy(&output.stderr).trim())) }
}

// ---- DNS

enum Dns {
    /// Autark's own choice, in `autark-dns.conf`: take our override away.
    Default,
    /// What the network's DHCP says.
    Network,
    Custom(Vec<String>),
}

/// The settings drop-in as keys and values, in the order `[Resolve]` lists them.
fn parse_dropin(text: &str) -> BTreeMap<String, String> {
    text.lines().filter_map(|line| line.split_once('=')).map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned())).collect()
}

fn render_dropin(values: &BTreeMap<String, String>) -> String {
    let mut text = String::from("# Written by Settings (autark-settings-helper). Delete this file to go back to Autark's defaults.\n[Resolve]\n");
    for (key, value) in values {
        text.push_str(&format!("{key}={value}\n"));
    }
    text
}

fn resolved_set(changes: &[(&str, Option<String>)]) -> Result<String, String> {
    let path = Path::new(RESOLVED_DROPIN);
    let mut values = parse_dropin(&std::fs::read_to_string(path).unwrap_or_default());
    for (key, value) in changes {
        match value {
            Some(value) => values.insert((*key).to_owned(), value.clone()),
            None => values.remove(*key),
        };
    }
    if values.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        write_file(path, &render_dropin(&values))?;
    }
    command("systemctl", &["restart", "systemd-resolved"])
}

fn resolved_dns(mode: &Dns) -> Result<String, String> {
    match mode {
        Dns::Default => {
            network_dropins(false)?;
            resolved_set(&[("DNS", None), ("FallbackDNS", None)])?;
        }
        Dns::Network => {
            network_dropins(false)?;
            // An empty DNS= clears the one in autark-dns.conf; DHCP's servers then stand alone.
            resolved_set(&[("DNS", Some(String::new())), ("FallbackDNS", Some(String::new())), ("DNSOverTLS", Some("opportunistic".to_owned()))])?;
        }
        Dns::Custom(servers) => {
            network_dropins(true)?;
            resolved_set(&[("DNS", Some(servers.join(" "))), ("FallbackDNS", Some(String::new()))])?;
        }
    }
    Ok("DNS changed".into())
}

/// Tells every `.network` file to ignore the DNS servers DHCP and router advertisements hand out (or stops telling it).
fn network_dropins(ignore: bool) -> Result<(), String> {
    let entries = std::fs::read_dir(NETWORK_DIR).map_err(|error| format!("{NETWORK_DIR}: {error}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "network") {
            continue;
        }
        let folder: PathBuf = PathBuf::from(format!("{}.d", path.display()));
        let file = folder.join(NETWORK_DROPIN);
        if ignore {
            std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
            write_file(&file, "# Written by Settings (autark-settings-helper): use the DNS servers chosen there.\n[DHCPv4]\nUseDNS=no\n[DHCPv6]\nUseDNS=no\n[IPv6AcceptRA]\nUseDNS=no\n")?;
        } else if file.exists() {
            let _ = std::fs::remove_file(&file);
            let _ = std::fs::remove_dir(&folder);
        }
    }
    command("networkctl", &["reload"])?;
    Ok(())
}

// ---- running things

fn service(action: &str, unit: &str) -> Result<String, String> {
    let unit = format!("{unit}.service");
    match action {
        "enable" => {
            command("systemctl", &["enable", "--now", &unit])?;
            if unit == "bluetooth.service" {
                let _ = command("rfkill", &["unblock", "bluetooth"]);
            }
            Ok(format!("{unit} is on"))
        }
        _ => command("systemctl", &["disable", "--now", &unit]),
    }
}

/// Runs a program (no shell, a known PATH) and gives its output, or what it complained of.
fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).env_clear().env("PATH", PATH).env("LANG", "C.UTF-8").stdin(Stdio::null()).output().map_err(|error| format!("{program}: {error}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        text.push_str(&said);
        return Ok(text.trim_end().to_owned());
    }
    Err(format!("{program}: {}", if said.trim().is_empty() { text.trim() } else { said.trim() }))
}

/// Replaces a file whole, so nothing reads half of it.
fn write_file(path: &Path, text: &str) -> Result<(), String> {
    let result = (|| {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("new");
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    result.map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_servers_are_addresses() {
        assert!(valid_server("9.9.9.9").is_ok());
        assert!(valid_server("2620:fe::fe#dns.quad9.net").is_ok());
        assert!(valid_server("dns.quad9.net").is_err());
        assert!(valid_server("9.9.9.9#bad name").is_err());
        assert!(valid_server("9.9.9.9#a;b").is_err());
        assert!(valid_server("9.9.9.9\nDNSSEC=no").is_err());
    }

    #[test]
    fn names_are_strict() {
        assert!(valid_name("maria"));
        assert!(valid_name("_svc-1"));
        assert!(!valid_name(""));
        assert!(!valid_name("Maria"));
        assert!(!valid_name("1maria"));
        assert!(!valid_name("a:b"));
        assert!(!valid_name("-rf"));
        assert!(!valid_name(&"a".repeat(33)));
    }

    #[test]
    fn full_names_cannot_break_the_passwd_line() {
        assert!(valid_full_name("Maria Santos").is_ok());
        assert!(valid_full_name("a:0:0:root").is_err());
        assert!(valid_full_name("a,b").is_err());
        assert!(valid_full_name("a\nb").is_err());
        assert!(valid_full_name(&"x".repeat(61)).is_err());
    }

    #[test]
    fn passwords_keep_to_one_line() {
        assert!(valid_password("hunter2: ok?").is_ok());
        assert!(valid_password("").is_err());
        assert!(valid_password("a\nb").is_err());
    }

    #[test]
    fn zones_are_files_under_zoneinfo() {
        assert!(check_zone("Asia/Manila").is_ok());
        assert!(check_zone("../etc/passwd").is_err());
        assert!(check_zone("/etc/passwd").is_err());
        assert!(check_zone("Not/AZone").is_err());
    }

    #[test]
    fn only_known_verbs_run() {
        assert!(run(&["rm", "-rf", "/"], "").is_err());
        assert!(run(&["service", "enable", "sshd"], "").is_err());
        assert!(run(&["resolved-tls", "maybe"], "").is_err());
        assert!(run(&["resolved-dns", "custom", "not-an-ip"], "").is_err());
        assert!(run(&["resolved-dns", "custom"], "").is_err());
        assert!(run(&["user-shell", "root", "/bin/sh"], "").is_err());
        assert!(run(&["user-remove", "root", "keep-home"], "").is_err());
    }

    #[test]
    fn the_dropin_round_trips() {
        let mut values = BTreeMap::new();
        values.insert("DNS".to_owned(), "1.1.1.1#cloudflare-dns.com".to_owned());
        values.insert("DNSOverTLS".to_owned(), "yes".to_owned());
        let text = render_dropin(&values);
        assert!(text.contains("[Resolve]\nDNS=1.1.1.1#cloudflare-dns.com\nDNSOverTLS=yes\n"));
        assert_eq!(parse_dropin(&text), values);
    }
}
