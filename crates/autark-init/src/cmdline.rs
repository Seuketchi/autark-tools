//! The kernel command line, as the boot image sets it: `rd.luks.name=<UUID>=<name>` for each encrypted volume,
//! `rd.luks.options=<UUID>=<options>` for how to open it (`tpm2-device=auto` is what asks for the TPM and its PIN), and
//! `root=`, `rw` or `ro`, `rootfstype=`, `rootflags=`, `init=`. These are the same names dracut and systemd-cryptsetup read, so one
//! command line boots either initramfs.

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cmdline {
    /// The encrypted volumes to open, in the order they were named.
    pub luks: Vec<Luks>,
    pub root: Option<String>,
    pub root_fstype: Option<String>,
    pub root_flags: Option<String>,
    /// `ro` unless `rw` came later (the last of the two wins, as in the kernel).
    pub read_only: bool,
    pub init: Option<String>,
    /// How long to wait for a device to appear, in seconds (`rd.timeout`).
    pub timeout: u64,
    pub debug: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Luks {
    /// Lower case, without a `luks-` or `UUID=` prefix.
    pub uuid: String,
    /// The name under `/dev/mapper`.
    pub name: String,
    pub options: Options,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `tpm2-device=…`: try the TPM2 token (and ask for its PIN) before the passphrase.
    pub tpm2: bool,
    pub discard: bool,
    pub no_read_workqueue: bool,
    pub no_write_workqueue: bool,
    /// Options this doesn't act on; they're reported, not an error.
    pub ignored: Vec<String>,
}

const DEFAULT_TIMEOUT: u64 = 30;

/// The words of a command line: split on spaces, with double quotes keeping a word together.
fn words(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let (mut current, mut quoted, mut started) = (String::new(), false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    found.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        found.push(current);
    }
    found
}

/// A UUID as written on the command line (`UUID=…`, `luks-…`, upper case) in the form libcryptsetup prints.
pub fn normalise_uuid(text: &str) -> String {
    let text = text.strip_prefix("UUID=").or_else(|| text.strip_prefix("uuid=")).unwrap_or(text);
    text.strip_prefix("luks-").unwrap_or(text).to_ascii_lowercase()
}

fn options(list: &str) -> Options {
    let mut parsed = Options::default();
    for option in list.split(',').filter(|option| !option.is_empty()) {
        match option.split_once('=').map_or(option, |(name, _)| name) {
            "tpm2-device" => parsed.tpm2 = true,
            "discard" => parsed.discard = true,
            "no-read-workqueue" => parsed.no_read_workqueue = true,
            "no-write-workqueue" => parsed.no_write_workqueue = true,
            _ => parsed.ignored.push(option.to_owned()),
        }
    }
    parsed
}

pub fn parse(line: &str) -> Cmdline {
    let mut parsed = Cmdline { read_only: true, timeout: DEFAULT_TIMEOUT, ..Cmdline::default() };
    // `rd.luks.options=<UUID>=…` can come before or after its `rd.luks.name=`, so collect both, then join them.
    let mut per_volume: Vec<(String, Options)> = Vec::new();
    let mut for_all: Option<Options> = None;
    for word in words(line) {
        let (key, value) = word.split_once('=').map_or((word.as_str(), ""), |(key, value)| (key, value));
        match key {
            "rd.luks.name" => {
                if let Some((uuid, name)) = value.rsplit_once('=') {
                    parsed.luks.push(Luks { uuid: normalise_uuid(uuid), name: name.to_owned(), options: Options::default() });
                }
            }
            "rd.luks.options" => match value.strip_prefix("UUID=").or_else(|| value.strip_prefix("uuid=")).unwrap_or(value).split_once('=') {
                // `<UUID>=<options>`; a bare list (`discard`) applies to every volume. A UUID has no commas and the
                // first `=` of an option list is the one after the option's name, so look at what's before it.
                Some((first, rest)) if looks_like_uuid(first) => per_volume.push((normalise_uuid(first), options(rest))),
                _ => for_all = Some(options(value)),
            },
            "root" => parsed.root = Some(value.to_owned()),
            "rootfstype" => parsed.root_fstype = Some(value.to_owned()),
            "rootflags" => parsed.root_flags = Some(value.to_owned()),
            "init" => parsed.init = Some(value.to_owned()),
            "rd.timeout" => parsed.timeout = value.parse().unwrap_or(DEFAULT_TIMEOUT),
            "ro" => parsed.read_only = true,
            "rw" => parsed.read_only = false,
            "rd.debug" | "autark.debug" => parsed.debug = true,
            _ => {}
        }
    }
    for volume in &mut parsed.luks {
        if let Some((_, found)) = per_volume.iter().find(|(uuid, _)| *uuid == volume.uuid) {
            volume.options = Options { ignored: found.ignored.clone(), ..Options { tpm2: found.tpm2, discard: found.discard, no_read_workqueue: found.no_read_workqueue, no_write_workqueue: found.no_write_workqueue, ignored: Vec::new() } };
        } else if let Some(found) = &for_all {
            volume.options = Options { tpm2: found.tpm2, discard: found.discard, no_read_workqueue: found.no_read_workqueue, no_write_workqueue: found.no_write_workqueue, ignored: found.ignored.clone() };
        }
    }
    parsed
}

/// Eight, four, four, four and twelve hex digits with dashes, after any `UUID=` or `luks-`.
fn looks_like_uuid(text: &str) -> bool {
    let text = normalise_uuid(text);
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5 && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(length, part)| part.len() == *length && part.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "3f0a1b2c-4d5e-4f60-8a7b-9c8d7e6f5a4b";

    #[test]
    fn the_boot_images_command_line() {
        let line = format!("rd.luks.name={UUID}=autark rd.luks.options={UUID}=tpm2-device=auto root=/dev/mapper/autark rw");
        let cmdline = parse(&line);
        assert_eq!(cmdline.luks.len(), 1);
        assert_eq!(cmdline.luks[0].uuid, UUID);
        assert_eq!(cmdline.luks[0].name, "autark");
        assert!(cmdline.luks[0].options.tpm2);
        assert_eq!(cmdline.root.as_deref(), Some("/dev/mapper/autark"));
        assert!(!cmdline.read_only);
        assert_eq!(cmdline.timeout, 30);
    }

    #[test]
    fn options_can_come_before_the_name() {
        let line = format!("rd.luks.options={UUID}=tpm2-device=auto,discard rd.luks.name={UUID}=autark");
        let cmdline = parse(&line);
        assert!(cmdline.luks[0].options.tpm2 && cmdline.luks[0].options.discard);
    }

    #[test]
    fn upper_case_and_prefixes_are_the_same_volume() {
        let upper = UUID.to_ascii_uppercase();
        let line = format!("rd.luks.name=luks-{upper}=autark rd.luks.options=UUID={UUID}=discard");
        let cmdline = parse(&line);
        assert_eq!(cmdline.luks[0].uuid, UUID);
        assert!(cmdline.luks[0].options.discard);
    }

    #[test]
    fn a_bare_option_list_is_for_every_volume() {
        let other = "11111111-2222-3333-4444-555555555555";
        let line = format!("rd.luks.name={UUID}=autark rd.luks.name={other}=home rd.luks.options=discard,tpm2-device=auto");
        let cmdline = parse(&line);
        assert!(cmdline.luks.iter().all(|volume| volume.options.discard && volume.options.tpm2));
    }

    #[test]
    fn a_volume_without_options_asks_for_a_passphrase_only() {
        let cmdline = parse(&format!("rd.luks.name={UUID}=autark root=/dev/mapper/autark"));
        assert!(!cmdline.luks[0].options.tpm2);
    }

    #[test]
    fn the_last_of_ro_and_rw_wins() {
        assert!(parse("root=/dev/x ro").read_only);
        assert!(!parse("ro rw").read_only);
        assert!(parse("rw ro").read_only);
        assert!(parse("").read_only);
    }

    #[test]
    fn unknown_options_are_kept_not_fatal() {
        let line = format!("rd.luks.name={UUID}=autark rd.luks.options={UUID}=tpm2-device=auto,tries=5,x-systemd.device-timeout=30");
        let ignored = &parse(&line).luks[0].options.ignored;
        assert_eq!(ignored, &["tries=5".to_owned(), "x-systemd.device-timeout=30".to_owned()]);
    }

    #[test]
    fn quotes_keep_a_word_together_and_other_words_are_left_alone() {
        let cmdline = parse(r#"quiet splash root="/dev/mapper/my root" rootfstype=ext4 rootflags=noatime,discard init=/usr/lib/systemd/systemd rd.timeout=90"#);
        assert_eq!(cmdline.root.as_deref(), Some("/dev/mapper/my root"));
        assert_eq!(cmdline.root_fstype.as_deref(), Some("ext4"));
        assert_eq!(cmdline.root_flags.as_deref(), Some("noatime,discard"));
        assert_eq!(cmdline.init.as_deref(), Some("/usr/lib/systemd/systemd"));
        assert_eq!(cmdline.timeout, 90);
    }

    #[test]
    fn a_broken_timeout_falls_back() {
        assert_eq!(parse("rd.timeout=soon").timeout, 30);
    }
}
