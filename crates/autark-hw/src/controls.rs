//! The controls and gauges a laptop offers through the kernel's standard interfaces: backlights and LEDs, the
//! battery, fans and temperatures, radios and the platform power profile. A laptop that has them in the standard
//! places is handled with no knowledge of its make; what it keeps somewhere of its own is a vendor profile's job.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slider {
    pub label: String,
    /// `backlight` or `leds`, the subsystems logind sets for the active user without root.
    pub subsystem: &'static str,
    pub name: String,
    pub value: u32,
    pub max: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    pub percent: u8,
    pub path: PathBuf,
    /// Whether this user may change it; most often only root may.
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Battery {
    pub name: String,
    pub percent: u8,
    pub status: String,
    pub cycles: Option<u32>,
    /// What it holds now against what it held new, as a percentage.
    pub health: Option<u8>,
    pub limit: Option<Limit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Radio {
    pub index: u32,
    pub kind: String,
    pub name: String,
    pub soft_blocked: bool,
    pub hard_blocked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub current: String,
    pub choices: Vec<String>,
    pub path: PathBuf,
    pub writable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub vendor: String,
    pub product: String,
    pub brightness: Vec<Slider>,
    pub lights: Vec<Slider>,
    pub batteries: Vec<Battery>,
    pub fans: Vec<Reading>,
    pub temperatures: Vec<Reading>,
    pub radios: Vec<Radio>,
    pub profile: Option<Profile>,
}

fn text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|text| text.trim().to_owned())
}

fn number<T: std::str::FromStr>(path: &Path) -> Option<T> {
    text(path)?.parse().ok()
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whether this user can write `path`.
pub fn writable(path: &Path) -> bool {
    fs::OpenOptions::new().write(true).open(path).is_ok()
}

/// The LEDs worth offering: a keyboard's backlight, in whatever way the driver names it. Caps Lock and the like are
/// indicators, and a Wi-Fi LED is the radio's.
fn is_light(name: &str) -> bool {
    let name = name.to_lowercase();
    (name.contains("kbd") || name.contains("keyboard") || name.contains("backlight")) && !name.contains("capslock") && !name.contains("numlock") && !name.contains("scrolllock")
}

fn label_for_light(name: &str) -> String {
    if name.to_lowercase().contains("kbd") || name.to_lowercase().contains("keyboard") { "Keyboard backlight".to_owned() } else { name.to_owned() }
}

/// What it's like to measure `microdegrees`-style hwmon temperatures: thousandths of a degree.
fn degrees(path: &Path) -> Option<String> {
    number::<i64>(path).map(|milli| format!("{:.0} °C", milli as f64 / 1000.0))
}

pub fn detect(sys: &Path) -> Controls {
    let mut controls = Controls::default();
    let dmi = sys.join("class/dmi/id");
    controls.vendor = text(&dmi.join("sys_vendor")).unwrap_or_default();
    controls.product = text(&dmi.join("product_name")).unwrap_or_default();

    for path in entries(&sys.join("class/backlight")) {
        if let (Some(value), Some(max)) = (number(&path.join("brightness")), number::<u32>(&path.join("max_brightness")))
            && max > 0
        {
            controls.brightness.push(Slider { label: "Screen brightness".to_owned(), subsystem: "backlight", name: file_name(&path), value, max });
        }
    }
    for path in entries(&sys.join("class/leds")) {
        let name = file_name(&path);
        if is_light(&name)
            && let (Some(value), Some(max)) = (number(&path.join("brightness")), number::<u32>(&path.join("max_brightness")))
            && max > 0
        {
            controls.lights.push(Slider { label: label_for_light(&name), subsystem: "leds", name, value, max });
        }
    }

    for path in entries(&sys.join("class/power_supply")) {
        if text(&path.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let Some(percent) = number::<u8>(&path.join("capacity")) else { continue };
        let health = [("energy_full", "energy_full_design"), ("charge_full", "charge_full_design")]
            .iter()
            .find_map(|(now, new)| Some((number::<f64>(&path.join(now))?, number::<f64>(&path.join(new))?)))
            .filter(|(_, new)| *new > 0.0)
            .map(|(now, new)| (now / new * 100.0).round().min(100.0) as u8);
        let limit_path = path.join("charge_control_end_threshold");
        let limit = number::<u8>(&limit_path).map(|percent| Limit { percent, writable: writable(&limit_path), path: limit_path });
        controls.batteries.push(Battery { name: file_name(&path), percent, status: text(&path.join("status")).unwrap_or_default(), cycles: number(&path.join("cycle_count")).filter(|cycles| *cycles > 0), health, limit });
    }

    for path in entries(&sys.join("class/hwmon")) {
        let chip = text(&path.join("name")).unwrap_or_default();
        for file in entries(&path) {
            let name = file_name(&file);
            if let Some(n) = name.strip_prefix("fan").and_then(|rest| rest.strip_suffix("_input"))
                && let Some(rpm) = number::<u32>(&file)
                && rpm > 0
            {
                controls.fans.push(Reading { label: format!("Fan {n}"), value: format!("{rpm} rpm") });
            }
            // One temperature that stands for each chip: the CPU package, the SSD's composite, the ACPI zone.
            if let Some(n) = name.strip_prefix("temp").and_then(|rest| rest.strip_suffix("_input")) {
                let label = text(&path.join(format!("temp{n}_label"))).unwrap_or_default();
                let wanted = match chip.as_str() {
                    "coretemp" => label == "Package id 0",
                    "nvme" => label == "Composite",
                    "acpitz" => n == "1",
                    _ => false,
                };
                if wanted && let Some(value) = degrees(&file) {
                    let what = match chip.as_str() {
                        "coretemp" => "Processor",
                        "nvme" => "SSD",
                        _ => "Chassis",
                    };
                    controls.temperatures.push(Reading { label: what.to_owned(), value });
                }
            }
        }
    }

    for path in entries(&sys.join("class/rfkill")) {
        let index = file_name(&path).trim_start_matches("rfkill").parse().unwrap_or(0);
        controls.radios.push(Radio {
            index,
            kind: text(&path.join("type")).unwrap_or_default(),
            name: text(&path.join("name")).unwrap_or_default(),
            soft_blocked: number::<u8>(&path.join("soft")) == Some(1),
            hard_blocked: number::<u8>(&path.join("hard")) == Some(1),
        });
    }

    let profile_path = sys.join("firmware/acpi/platform_profile");
    if let Some(current) = text(&profile_path) {
        let choices = text(&sys.join("firmware/acpi/platform_profile_choices")).map(|text| text.split_whitespace().map(str::to_owned).collect()).unwrap_or_default();
        controls.profile = Some(Profile { current, choices, writable: writable(&profile_path), path: profile_path });
    }
    controls
}

/// Switches a radio off (`true`) or on, through `/dev/rfkill` as the `rfkill` tool does: an 8-byte event.
pub fn set_radio(index: u32, blocked: bool) -> std::io::Result<()> {
    set_radio_at(Path::new("/dev/rfkill"), index, blocked)
}

fn set_radio_at(device: &Path, index: u32, blocked: bool) -> std::io::Result<()> {
    // struct rfkill_event { __u32 idx; __u8 type; __u8 op; __u8 soft; __u8 hard; }, op RFKILL_OP_CHANGE = 2.
    let mut event = [0u8; 8];
    event[..4].copy_from_slice(&index.to_ne_bytes());
    event[5] = 2;
    event[6] = u8::from(blocked);
    fs::OpenOptions::new().write(true).open(device)?.write_all(&event)
}

/// Writes a sysfs value that this user may write, such as the power profile or a charge limit.
pub fn write_value(path: &Path, value: &str) -> std::io::Result<()> {
    fs::OpenOptions::new().write(true).open(path)?.write_all(value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, path: &str, content: &str) {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    fn fake_sys() -> PathBuf {
        let root = std::env::temp_dir().join(format!("autark-hw-sys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        put(&root, "class/dmi/id/sys_vendor", "Micro-Star International Co., Ltd.\n");
        put(&root, "class/dmi/id/product_name", "Cyborg 15 A13UCK\n");
        put(&root, "class/backlight/intel_backlight/brightness", "48000\n");
        put(&root, "class/backlight/intel_backlight/max_brightness", "96000\n");
        put(&root, "class/leds/input3::capslock/brightness", "0\n");
        put(&root, "class/leds/input3::capslock/max_brightness", "1\n");
        put(&root, "class/leds/platform::kbd_backlight/brightness", "1\n");
        put(&root, "class/leds/platform::kbd_backlight/max_brightness", "3\n");
        put(&root, "class/power_supply/BAT1/type", "Battery\n");
        put(&root, "class/power_supply/BAT1/capacity", "80\n");
        put(&root, "class/power_supply/BAT1/status", "Not charging\n");
        put(&root, "class/power_supply/BAT1/cycle_count", "212\n");
        put(&root, "class/power_supply/BAT1/energy_full", "45000000\n");
        put(&root, "class/power_supply/BAT1/energy_full_design", "50000000\n");
        put(&root, "class/power_supply/BAT1/charge_control_end_threshold", "100\n");
        put(&root, "class/power_supply/ADP1/type", "Mains\n");
        put(&root, "class/hwmon/hwmon4/name", "msi_wmi_platform\n");
        put(&root, "class/hwmon/hwmon4/fan1_input", "3555\n");
        put(&root, "class/hwmon/hwmon4/fan2_input", "0\n");
        put(&root, "class/hwmon/hwmon5/name", "coretemp\n");
        put(&root, "class/hwmon/hwmon5/temp1_label", "Package id 0\n");
        put(&root, "class/hwmon/hwmon5/temp1_input", "62000\n");
        put(&root, "class/hwmon/hwmon5/temp2_label", "Core 0\n");
        put(&root, "class/hwmon/hwmon5/temp2_input", "55000\n");
        put(&root, "class/rfkill/rfkill1/type", "wlan\n");
        put(&root, "class/rfkill/rfkill1/name", "phy0\n");
        put(&root, "class/rfkill/rfkill1/soft", "1\n");
        put(&root, "class/rfkill/rfkill1/hard", "0\n");
        put(&root, "firmware/acpi/platform_profile", "balanced\n");
        put(&root, "firmware/acpi/platform_profile_choices", "low-power balanced performance\n");
        root
    }

    #[test]
    fn a_laptop_in_the_standard_places_is_understood() {
        let sys = fake_sys();
        let controls = detect(&sys);
        assert_eq!((controls.vendor.as_str(), controls.product.as_str()), ("Micro-Star International Co., Ltd.", "Cyborg 15 A13UCK"));
        assert_eq!(controls.brightness, [Slider { label: "Screen brightness".into(), subsystem: "backlight", name: "intel_backlight".into(), value: 48000, max: 96000 }]);
        // Caps Lock's LED isn't a light to offer; the keyboard's is.
        assert_eq!(controls.lights.len(), 1);
        assert_eq!((controls.lights[0].label.as_str(), controls.lights[0].name.as_str(), controls.lights[0].max), ("Keyboard backlight", "platform::kbd_backlight", 3));
        let battery = &controls.batteries[0];
        assert_eq!((battery.percent, battery.status.as_str(), battery.cycles, battery.health), (80, "Not charging", Some(212), Some(90)));
        assert_eq!(battery.limit.as_ref().map(|limit| limit.percent), Some(100));
        // Only fans that turn, and one temperature for each chip.
        assert_eq!(controls.fans, [Reading { label: "Fan 1".into(), value: "3555 rpm".into() }]);
        assert_eq!(controls.temperatures, [Reading { label: "Processor".into(), value: "62 °C".into() }]);
        assert_eq!(controls.radios.len(), 1);
        assert!(controls.radios[0].soft_blocked && !controls.radios[0].hard_blocked && controls.radios[0].index == 1);
        let profile = controls.profile.as_ref().unwrap();
        assert_eq!((profile.current.as_str(), profile.choices.len()), ("balanced", 3));
        fs::remove_dir_all(&sys).unwrap();
    }

    #[test]
    fn a_laptop_with_nothing_standard_has_nothing_listed() {
        let empty = std::env::temp_dir().join(format!("autark-hw-empty-{}", std::process::id()));
        fs::create_dir_all(&empty).unwrap();
        let controls = detect(&empty);
        assert_eq!(controls, Controls::default());
        fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn a_radio_is_switched_by_an_eight_byte_event() {
        let device = std::env::temp_dir().join(format!("autark-hw-rfkill-{}", std::process::id()));
        fs::write(&device, "").unwrap();
        set_radio_at(&device, 3, true).unwrap();
        assert_eq!(fs::read(&device).unwrap(), [3, 0, 0, 0, 0, 2, 1, 0]);
        fs::remove_file(&device).unwrap();
    }

    #[test]
    fn only_keyboard_lights_are_lights() {
        assert!(is_light("platform::kbd_backlight") && is_light("rgb:kbd_backlight") && is_light("asus::kbd_backlight"));
        assert!(!is_light("input3::capslock") && !is_light("phy0-led") && !is_light("input3::numlock"));
    }
}
