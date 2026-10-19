//! What the helper will do, and nothing else. It runs as root on behalf of anyone at the keyboard, so every request is
//! checked here against a short list: a few sysfs files that are safe to set, each with the values it takes, and kernel
//! modules that drive hardware that's actually present. The caller's say-so is never trusted: paths are rebuilt from
//! checked parts, and values are read back as what they are.

use std::path::{Component, Path, PathBuf};

fn text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|text| text.trim().to_owned())
}

/// A power-supply name such as `BAT0` or `BAT1`: letters, digits and a few signs, never a path.
fn supply_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The words of a sysfs choice list, whether `a b c` or `a [b] c` with the current one in brackets.
fn choices(list: &str) -> Vec<String> {
    list.split_whitespace().map(|word| word.trim_matches(|c| c == '[' || c == ']').to_owned()).collect()
}

/// Turns a request to set `path` to `value` into the file to write and what to write, or says why not. `sys` is where
/// `/sys` is (a test points it elsewhere); `path` is as the caller gave it, starting `/sys/`.
pub fn setting(sys: &Path, path: &str, value: &str) -> Result<(PathBuf, String), String> {
    let rel = path.strip_prefix("/sys/").ok_or("only files under /sys can be set")?;
    let parts: Vec<&str> = rel.split('/').collect();
    // Plain names only: no `..`, no empty parts from `//`, nothing that could step out of the folder it names.
    if parts.iter().any(|part| part.is_empty() || *part == "." || *part == ".." || part.contains('\0')) || Path::new(rel).components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("that isn't a plain path".to_owned());
    }
    let value = value.trim();
    match parts.as_slice() {
        ["firmware", "acpi", "platform_profile"] => {
            let allowed = choices(&text(&sys.join("firmware/acpi/platform_profile_choices")).ok_or("this machine has no power profiles")?);
            if !allowed.iter().any(|choice| choice == value) {
                return Err(format!("“{value}” isn't one of: {}", allowed.join(", ")));
            }
            Ok((sys.join(rel), value.to_owned()))
        }
        ["class", "power_supply", name, attribute @ ("charge_control_end_threshold" | "charge_control_start_threshold" | "charge_behaviour")] => {
            if !supply_name(name) {
                return Err("that isn't a power supply's name".to_owned());
            }
            let dir = sys.join("class/power_supply").join(name);
            if text(&dir.join("type")).as_deref() != Some("Battery") {
                return Err("that isn't a battery".to_owned());
            }
            if !dir.join(attribute).exists() {
                return Err("this battery doesn't have that setting".to_owned());
            }
            let written = match *attribute {
                // Stopping short of full is the point; below a fifth a battery is no use, and 100 means no limit.
                "charge_control_end_threshold" => {
                    let percent: u8 = value.parse().map_err(|_| "a charge limit is a number from 20 to 100".to_owned())?;
                    if !(20..=100).contains(&percent) {
                        return Err("a charge limit is a number from 20 to 100".to_owned());
                    }
                    percent.to_string()
                }
                "charge_control_start_threshold" => {
                    let percent: u8 = value.parse().map_err(|_| "a charge start is a number from 0 to 95".to_owned())?;
                    if percent > 95 {
                        return Err("a charge start is a number from 0 to 95".to_owned());
                    }
                    percent.to_string()
                }
                // Only the two ends of "normal": let it charge, or stop it charging. Forcing a discharge is left out.
                _ => {
                    if !matches!(value, "auto" | "inhibit-charge") {
                        return Err("charging can be set to auto or inhibit-charge".to_owned());
                    }
                    let offered = choices(&text(&dir.join("charge_behaviour")).unwrap_or_default());
                    if !offered.iter().any(|choice| choice == value) {
                        return Err(format!("this battery doesn't offer “{value}”"));
                    }
                    value.to_owned()
                }
            };
            Ok((dir.join(attribute), written))
        }
        _ => Err("that setting isn't one this helper changes".to_owned()),
    }
}

/// Whether `name` could be a module name, and can't be taken for an option to the program that loads it.
pub fn module_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sys(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("autark-hw-helper-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let put = |path: &str, content: &str| {
            let full = root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        };
        put("firmware/acpi/platform_profile", "balanced\n");
        put("firmware/acpi/platform_profile_choices", "low-power balanced performance\n");
        put("class/power_supply/BAT1/type", "Battery\n");
        put("class/power_supply/BAT1/charge_control_end_threshold", "100\n");
        put("class/power_supply/BAT1/charge_control_start_threshold", "0\n");
        put("class/power_supply/BAT1/charge_behaviour", "[auto] inhibit-charge force-discharge\n");
        put("class/power_supply/ADP1/type", "Mains\n");
        put("class/power_supply/ADP1/charge_control_end_threshold", "100\n");
        root
    }

    #[test]
    fn the_settings_on_the_list_are_allowed_with_their_values() {
        let root = sys("allowed");
        assert_eq!(setting(&root, "/sys/firmware/acpi/platform_profile", " performance\n").unwrap().1, "performance");
        let (file, value) = setting(&root, "/sys/class/power_supply/BAT1/charge_control_end_threshold", "80").unwrap();
        assert_eq!((file, value.as_str()), (root.join("class/power_supply/BAT1/charge_control_end_threshold"), "80"));
        assert_eq!(setting(&root, "/sys/class/power_supply/BAT1/charge_control_start_threshold", "75").unwrap().1, "75");
        assert_eq!(setting(&root, "/sys/class/power_supply/BAT1/charge_behaviour", "inhibit-charge").unwrap().1, "inhibit-charge");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn values_outside_what_a_setting_takes_are_refused() {
        let root = sys("values");
        for (path, value) in [
            ("/sys/firmware/acpi/platform_profile", "turbo"),
            ("/sys/firmware/acpi/platform_profile", ""),
            ("/sys/class/power_supply/BAT1/charge_control_end_threshold", "5"),
            ("/sys/class/power_supply/BAT1/charge_control_end_threshold", "101"),
            ("/sys/class/power_supply/BAT1/charge_control_end_threshold", "eighty"),
            ("/sys/class/power_supply/BAT1/charge_control_end_threshold", "80; rm -rf /"),
            ("/sys/class/power_supply/BAT1/charge_control_start_threshold", "96"),
            ("/sys/class/power_supply/BAT1/charge_behaviour", "force-discharge"),
        ] {
            assert!(setting(&root, path, value).is_err(), "{path} = {value}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn paths_off_the_list_or_trying_to_get_out_are_refused() {
        let root = sys("paths");
        for path in [
            "/etc/passwd",
            "/sys/class/power_supply/BAT1/../../../../etc/shadow",
            "/sys/class/power_supply/../power_supply/BAT1/charge_control_end_threshold",
            "/sys/class/power_supply//BAT1/charge_control_end_threshold",
            "/sys/class/power_supply/ADP1/charge_control_end_threshold",
            "/sys/class/power_supply/BAT9/charge_control_end_threshold",
            "/sys/class/power_supply/BAT1/uevent",
            "/sys/kernel/mm/ksm/run",
            "/sys/class/backlight/intel_backlight/brightness",
            "sys/firmware/acpi/platform_profile",
        ] {
            assert!(setting(&root, path, "80").is_err(), "{path}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn module_names_cannot_be_options() {
        assert!(module_name("nouveau") && module_name("snd_hda_intel") && module_name("btusb") && module_name("hid-generic"));
        assert!(!module_name("") && !module_name("-r") && !module_name("--force") && !module_name("a b") && !module_name("a;b") && !module_name("../x"));
    }
}
