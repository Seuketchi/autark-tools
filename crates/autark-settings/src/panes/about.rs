//! About: the system's name from `/etc/os-release` under the Loop logo, then what it runs on, read from
//! `/proc` and `/sys` (the GPU named from `pci.ids` when it's installed).

use std::path::{Path, PathBuf};

use autark_common::theme;
use gtk::prelude::*;

use crate::{icon, ui};

/// The Loop, as the design draws it here.
const LOGO: &str = "M5.5 19.5L12 4.5l6.5 15C14.5 16 9.5 16 5.5 19.5z";

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |key: &str| os_field(&release, key);
    let autark = field("ID").as_deref() == Some("autark");
    let name = field("NAME").unwrap_or_else(|| "Autark".to_owned());
    let edition = if autark || field("ID").is_none() { Some("Linux From Scratch, systemd edition".to_owned()) } else { field("PRETTY_NAME").filter(|pretty| *pretty != name) };

    let top = gtk::Box::new(gtk::Orientation::Vertical, 10);
    top.add_css_class("about");
    top.set_halign(gtk::Align::Center);
    let logo = gtk::DrawingArea::builder().content_width(88).content_height(88).halign(gtk::Align::Center).build();
    logo.set_draw_func(|_, cr, _, _| {
        theme::set_source(cr, &theme::look().palette.accent, 1.0);
        icon::stroke(cr, LOGO, 88.0);
    });
    theme::redraw_on_change(&logo);
    top.append(&logo);
    let title = gtk::Label::new(Some(&name));
    title.add_css_class("about-name");
    top.append(&title);
    if let Some(edition) = edition {
        let note = ui::note(&edition);
        note.set_halign(gtk::Align::Center);
        top.append(&note);
    }
    column.append(&top);

    let group = ui::group();
    let rows = [
        ("Processor", processor(&std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default())),
        ("Graphics", graphics()),
        ("Memory", memory(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default(), &std::fs::read_to_string("/proc/swaps").unwrap_or_default())),
        ("Storage", storage()),
        ("Kernel", std::fs::read_to_string("/proc/sys/kernel/osrelease").ok().map(|release| format!("Linux {}", release.trim()))),
        ("Compositor", compositor()),
        ("Desktop shell", Some(format!("autark-tools {} (GTK {}.{} + Rust)", env!("CARGO_PKG_VERSION"), gtk::major_version(), gtk::minor_version()))),
    ];
    for (label, value) in rows {
        let value = gtk::Label::new(Some(value.as_deref().unwrap_or("Unknown")));
        value.add_css_class("value");
        value.set_selectable(true);
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);
        group.append(&ui::row(label, None, &value));
    }
    column.append(&group);
    page.upcast()
}

/// A value from os-release, quotes taken off.
fn os_field(release: &str, key: &str) -> Option<String> {
    release.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('=')).map(|value| value.trim().trim_matches('"').trim_matches('\'').to_owned()).filter(|value| !value.is_empty())
}

/// The CPU's name without the marks and clock: "11th Gen Intel(R) Core(TM) i5-1135G7 @ 2.40GHz" becomes
/// "Intel Core i5-1135G7".
fn processor(cpuinfo: &str) -> Option<String> {
    let model = cpuinfo.lines().find_map(|line| line.strip_prefix("model name")?.split_once(':').map(|(_, value)| value.trim().to_owned()))?;
    let model = model.split(" @ ").next().unwrap_or(&model).replace("(R)", "").replace("(TM)", "").replace(" CPU", "").replace(" Processor", "");
    let words: Vec<&str> = model.split_whitespace().collect();
    // "11th Gen" says what the model number already does.
    let words = match words.as_slice() {
        [generation, "Gen", rest @ ..] if generation.ends_with("th") || generation.ends_with("nd") || generation.ends_with("rd") || generation.ends_with("st") => rest.to_vec(),
        _ => words,
    };
    (!words.is_empty()).then(|| words.join(" "))
}

/// The display GPUs, by name from pci.ids, else by driver.
fn graphics() -> Option<String> {
    let ids = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"].iter().find_map(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    let mut names = Vec::new();
    let mut cards: Vec<PathBuf> = std::fs::read_dir("/sys/class/drm").into_iter().flatten().flatten().map(|entry| entry.path()).collect();
    cards.sort();
    for card in cards {
        let file = card.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        if !file.starts_with("card") || file.contains('-') {
            continue;
        }
        let read = |name: &str| std::fs::read_to_string(card.join("device").join(name)).ok().map(|text| text.trim().trim_start_matches("0x").to_lowercase());
        let name = match (read("vendor"), read("device")) {
            (Some(vendor), Some(device)) => pci_name(&ids, &vendor, &device),
            _ => None,
        };
        let name = name.or_else(|| std::fs::read_link(card.join("device/driver")).ok().and_then(|driver| driver.file_name().map(|name| format!("{} graphics", name.to_string_lossy()))));
        if let Some(name) = name.filter(|name| !names.contains(name)) {
            names.push(name);
        }
    }
    (!names.is_empty()).then(|| names.join(", "))
}

/// A device's name from pci.ids, with the vendor's first word: "Intel Iris Xe Graphics", taking the
/// marketing name in brackets when there is one.
fn pci_name(ids: &str, vendor: &str, device: &str) -> Option<String> {
    let mut lines = ids.lines().skip_while(|line| !line.starts_with(&format!("{vendor} ")));
    let vendor_name = lines.next()?.split_once("  ").map(|(_, name)| name.trim().to_owned())?;
    let device_name = lines.take_while(|line| line.starts_with('\t') || line.starts_with('#') || line.is_empty()).find_map(|line| {
        let line = line.strip_prefix('\t')?;
        if line.starts_with('\t') {
            return None;
        }
        let (id, name) = line.split_once("  ")?;
        (id == device).then(|| name.trim().to_owned())
    })?;
    let device_name = match (device_name.rfind('['), device_name.ends_with(']')) {
        (Some(open), true) => device_name[open + 1..device_name.len() - 1].to_owned(),
        _ => device_name,
    };
    let brand = vendor_name.split_whitespace().next().unwrap_or_default();
    let brand = match brand {
        "Advanced" => "AMD",
        "NVIDIA" | "Intel" => brand,
        other => other,
    };
    Some(if device_name.starts_with(brand) { device_name } else { format!("{brand} {device_name}") })
}

/// "30 GB, zram swap": the memory the kernel sees, in whole gigabytes, and the kinds of swap in use.
fn memory(meminfo: &str, swaps: &str) -> Option<String> {
    let kilobytes: u64 = meminfo.lines().find_map(|line| line.strip_prefix("MemTotal:")?.split_whitespace().next()?.parse().ok())?;
    let mut parts = vec![format!("{} GB", kilobytes / 1024 / 1024)];
    let mut kinds: Vec<(i64, &str)> = swaps
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let priority = fields.get(4)?.parse().unwrap_or(0);
            let kind = if fields[0].starts_with("/dev/zram") {
                "zram swap"
            } else if fields.get(1) == Some(&"file") {
                "swap file"
            } else {
                "swap partition"
            };
            Some((priority, kind))
        })
        .collect();
    kinds.sort_by_key(|(priority, _)| -priority);
    for (_, kind) in kinds {
        if !parts.iter().any(|part| part == kind) {
            parts.push(kind.to_owned());
        }
    }
    Some(parts.join(", "))
}

/// The disk the system is on: its model, size and the root's filesystem, and whether it's encrypted.
fn storage() -> Option<String> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let (device, filesystem) = root_mount(&mountinfo)?;
    let mut block = std::fs::canonicalize(format!("/sys/dev/block/{device}")).ok()?;
    let mut encrypted = false;
    // Down through device-mapper (LUKS, LVM) to the partition underneath.
    for _ in 0..4 {
        let Some(slave) = std::fs::read_dir(block.join("slaves")).ok().and_then(|mut slaves| slaves.next()).and_then(Result::ok) else { break };
        encrypted |= std::fs::read_to_string(block.join("dm/uuid")).is_ok_and(|uuid| uuid.starts_with("CRYPT-"));
        block = std::fs::canonicalize(slave.path()).ok()?;
    }
    if block.join("partition").exists() {
        block = block.parent()?.to_path_buf();
    }
    let model = read_trimmed(&block.join("device/model")).unwrap_or_else(|| block.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
    let bytes = read_trimmed(&block.join("size")).and_then(|sectors| sectors.parse::<u64>().ok()).map(|sectors| sectors * 512);
    let mut parts = vec![model];
    if let Some(bytes) = bytes {
        parts.push(size(bytes));
    }
    parts.push(filesystem);
    if encrypted {
        parts.push("encrypted".to_owned());
    }
    Some(parts.join(", "))
}

/// The root's device number and filesystem type, from mountinfo.
fn root_mount(mountinfo: &str) -> Option<(String, String)> {
    mountinfo.lines().rev().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(4) != Some(&"/") {
            return None;
        }
        let after = fields.iter().position(|field| *field == "-")?;
        Some((fields.get(2)?.to_string(), fields.get(after + 1)?.to_string()))
    })
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|text| text.trim().to_owned()).filter(|text| !text.is_empty())
}

/// A disk's size as its maker counts it: "512 GB", "1 TB".
fn size(bytes: u64) -> String {
    let gigabytes = bytes as f64 / 1e9;
    if gigabytes >= 1000.0 {
        let terabytes = gigabytes / 1000.0;
        if (terabytes - terabytes.round()).abs() < 0.1 { format!("{} TB", terabytes.round()) } else { format!("{terabytes:.1} TB") }
    } else {
        format!("{} GB", gigabytes.round())
    }
}

fn compositor() -> Option<String> {
    if std::env::var_os("WAYFIRE_SOCKET").is_some() {
        return Some("Wayfire".to_owned());
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok()?;
    desktop.split(':').next_back().map(str::to_owned).filter(|desktop| !desktop.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processors_lose_marks_and_clocks() {
        assert_eq!(processor("model name\t: 11th Gen Intel(R) Core(TM) i5-1135G7 @ 2.40GHz\n").as_deref(), Some("Intel Core i5-1135G7"));
        assert_eq!(processor("model name\t: AMD Ryzen 7 5800U with Radeon Graphics\n").as_deref(), Some("AMD Ryzen 7 5800U with Radeon Graphics"));
        assert_eq!(processor(""), None);
    }

    #[test]
    fn gpus_are_named_from_pci_ids() {
        let ids = "# comment\n8086  Intel Corporation\n\t0007  82379AB\n\t9a49  TigerLake-LP GT2 [Iris Xe Graphics]\n\t\t1025 1094  sub\n8087  Intel Corporation\n";
        assert_eq!(pci_name(ids, "8086", "9a49").as_deref(), Some("Intel Iris Xe Graphics"));
        assert_eq!(pci_name(ids, "8086", "0007").as_deref(), Some("Intel 82379AB"));
        assert_eq!(pci_name(ids, "8086", "ffff"), None);
    }

    #[test]
    fn memory_names_its_swap() {
        let swaps = "Filename\tType\tSize\tUsed\tPriority\n/swap.img file 8388604 0 -1\n/dev/zram0 partition 8388604 0 100\n";
        assert_eq!(memory("MemTotal:       32021512 kB\n", swaps).as_deref(), Some("30 GB, zram swap, swap file"));
        assert_eq!(memory("MemTotal: 8000000 kB\n", "Filename\tType\n").as_deref(), Some("7 GB"));
    }

    #[test]
    fn the_root_is_found_in_mountinfo() {
        let mountinfo = "1 0 0:1 / /proc rw - proc proc rw\n45 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw\n";
        assert_eq!(root_mount(mountinfo), Some(("259:2".to_owned(), "ext4".to_owned())));
    }

    #[test]
    fn sizes_read_as_makers_count() {
        assert_eq!(size(2_000_409_264 * 512), "1 TB");
        assert_eq!(size(512_110_190_592), "512 GB");
        assert_eq!(size(1_500_000_000_000), "1.5 TB");
    }

    #[test]
    fn os_release_fields_lose_quotes() {
        let release = "NAME=\"Autark\"\nID=autark\nPRETTY_NAME='Autark 1'\n";
        assert_eq!(os_field(release, "NAME").as_deref(), Some("Autark"));
        assert_eq!(os_field(release, "ID").as_deref(), Some("autark"));
        assert_eq!(os_field(release, "PRETTY_NAME").as_deref(), Some("Autark 1"));
        assert_eq!(os_field(release, "VERSION"), None);
    }
}
