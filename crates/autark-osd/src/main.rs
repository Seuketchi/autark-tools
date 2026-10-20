//! `autark-osd volume up|down|mute`, `autark-osd mic mute`, `autark-osd brightness up|down`, `autark-osd screens`, `autark-osd touchpad`,
//! `autark-osd bluetooth`: what the volume, mute, microphone, brightness, projection, touchpad and Bluetooth keys run. It makes the change and shows the result as a pop-up, so a press is visible: a transient notification (never kept in
//! the notification centre) that replaces the last one of its kind, with a bar, through `org.freedesktop.Notifications`.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use autark_common::backlight::Backlight;
use zbus::zvariant::Value;

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
const SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";
const VOLUME_STEP: &str = "5%";
const CELLS: usize = 20;
const SHOWN_MS: i32 = 1500;

/// `wpctl get-volume` output ("Volume: 0.75" or "Volume: 0.75 [MUTED]") as a percent and whether it's muted.
fn parse_volume(text: &str) -> Option<(u32, bool)> {
    let level: f64 = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(((level * 100.0).round() as u32, text.contains("[MUTED]")))
}

/// A bar of `CELLS` cells, filled in proportion to `percent`.
fn bar(percent: u32) -> String {
    let filled = ((percent.min(100) as usize * CELLS) + 50) / 100;
    "█".repeat(filled) + &"░".repeat(CELLS - filled)
}

/// The theme icon for a speaker at `percent`, or muted.
fn speaker_icon(percent: u32, muted: bool) -> &'static str {
    match (muted, percent) {
        (true, _) | (_, 0) => "audio-volume-muted-symbolic",
        (_, 1..=33) => "audio-volume-low-symbolic",
        (_, 34..=66) => "audio-volume-medium-symbolic",
        _ => "audio-volume-high-symbolic",
    }
}

fn wpctl(args: &[&str]) -> Option<String> {
    let output = Command::new("wpctl").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// What to show: a title, a bar line, an icon, a percent for the progress hint and a name that keeps one pop-up per kind.
struct Pop {
    kind: &'static str,
    icon: &'static str,
    title: String,
    body: String,
    percent: i32,
}

fn volume(action: &str) -> Result<Pop, String> {
    match action {
        "up" => drop(wpctl(&["set-volume", "-l", "1.0", SINK, &format!("{VOLUME_STEP}+")])),
        "down" => drop(wpctl(&["set-volume", SINK, &format!("{VOLUME_STEP}-")])),
        "mute" => drop(wpctl(&["set-mute", SINK, "toggle"])),
        _ => return Err("usage: autark-osd volume up|down|mute".to_owned()),
    }
    // Raising the volume with the speakers muted is asking to hear it.
    if action == "up" {
        drop(wpctl(&["set-mute", SINK, "0"]));
    }
    let (percent, muted) = wpctl(&["get-volume", SINK]).as_deref().and_then(parse_volume).ok_or("the sound system (PipeWire) didn't answer")?;
    Ok(Pop {
        kind: "volume",
        icon: speaker_icon(percent, muted),
        title: if muted { "Muted".to_owned() } else { format!("Volume {percent}%") },
        body: bar(if muted { 0 } else { percent }),
        percent: if muted { 0 } else { percent as i32 },
    })
}

fn microphone(action: &str) -> Result<Pop, String> {
    if action != "mute" {
        return Err("usage: autark-osd mic mute".to_owned());
    }
    drop(wpctl(&["set-mute", SOURCE, "toggle"]));
    let (_, muted) = wpctl(&["get-volume", SOURCE]).as_deref().and_then(parse_volume).ok_or("the sound system (PipeWire) didn't answer")?;
    Ok(Pop {
        kind: "mic",
        icon: if muted { "microphone-disabled-symbolic" } else { "microphone-sensitivity-high-symbolic" },
        title: if muted { "Microphone off".to_owned() } else { "Microphone on".to_owned() },
        body: String::new(),
        percent: -1,
    })
}

fn brightness(action: &str) -> Result<Pop, String> {
    if !matches!(action, "up" | "down") {
        return Err("usage: autark-osd brightness up|down".to_owned());
    }
    // The change itself is autark-brightness's, next to this program when both are installed together.
    let here = std::env::current_exe().ok().and_then(|path| path.parent().map(|dir| dir.join("autark-brightness")));
    let program = here.filter(|path| path.exists()).map_or_else(|| "autark-brightness".into(), |path| path.into_os_string());
    let status = Command::new(program).arg(action).stdin(Stdio::null()).status().map_err(|error| format!("autark-brightness: {error}"))?;
    if !status.success() {
        return Err("autark-brightness couldn't change the brightness".to_owned());
    }
    let percent = Backlight::find().ok_or("this laptop has no screen backlight")?.percent().round() as u32;
    Ok(Pop { kind: "brightness", icon: "display-brightness-symbolic", title: format!("Brightness {percent}%"), body: bar(percent), percent: percent as i32 })
}

/// The touchpads: each `inputN` under `<sys>/class/input` whose name says touchpad, with whether it is switched off (the kernel's `inhibited`).
fn touchpads(sys: &Path) -> Vec<(String, bool)> {
    let Ok(entries) = std::fs::read_dir(sys.join("class/input")) else { return Vec::new() };
    let mut found: Vec<(String, bool)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number = name.strip_prefix("input")?;
            if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let device = std::fs::read_to_string(entry.path().join("name")).ok()?;
            let off = std::fs::read_to_string(entry.path().join("inhibited")).ok()?;
            device.to_ascii_lowercase().contains("touchpad").then(|| (name, off.trim() == "1"))
        })
        .collect();
    found.sort();
    found
}

/// What the helper (`org.autark.Hardware1`, root, asks polkit) says when it refuses or can't be reached, in words.
fn bus_message(error: zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => "the hardware helper isn't installed".to_owned(),
        zbus::Error::MethodError(_, Some(message), _) => message,
        zbus::Error::FDO(error) => match *error {
            zbus::fdo::Error::Failed(message) | zbus::fdo::Error::InvalidArgs(message) | zbus::fdo::Error::AccessDenied(message) | zbus::fdo::Error::AuthFailed(message) => message,
            error => error.to_string(),
        },
        error => error.to_string(),
    }
}

fn write_setting(path: &str, value: &str) -> Result<(), String> {
    async_io::block_on(async {
        let connection = zbus::Connection::system().await.map_err(|error| format!("the system bus: {error}"))?;
        connection.call_method(Some("org.autark.Hardware1"), "/org/autark/Hardware1", Some("org.autark.Hardware1"), "WriteSetting", &(path, value)).await.map(drop).map_err(bus_message)
    })
}

/// The touchpad key: switches the touchpad off, or on again (`toggle`, `on` or `off`), through the root helper, which is the one thing that
/// may write the kernel's switch. What the pop-up says is what the kernel reports afterwards, not what was asked for.
fn touchpad(action: &str) -> Result<Pop, String> {
    let pads = touchpads(Path::new("/sys"));
    let pop = |off: bool, body: &str| Pop {
        kind: "touchpad",
        icon: if off { "touchpad-disabled-symbolic" } else { "input-touchpad-symbolic" },
        title: if off { "Touchpad off".to_owned() } else { "Touchpad on".to_owned() },
        body: body.to_owned(),
        percent: -1,
    };
    if pads.is_empty() {
        return Ok(Pop { kind: "touchpad", icon: "input-touchpad-symbolic", title: "Touchpad".to_owned(), body: "this laptop has no touchpad".to_owned(), percent: -1 });
    }
    let off_now = pads.iter().all(|(_, off)| *off);
    let want_off = match action {
        "" | "toggle" => !off_now,
        "off" => true,
        "on" => false,
        _ => return Err("usage: autark-osd touchpad [toggle|on|off]".to_owned()),
    };
    let mut failed = None;
    for (name, _) in &pads {
        if let Err(error) = write_setting(&format!("/sys/class/input/{name}/inhibited"), if want_off { "1" } else { "0" }) {
            failed = Some(error);
        }
    }
    let off = touchpads(Path::new("/sys")).iter().all(|(_, off)| *off);
    Ok(match failed {
        Some(error) => Pop { title: "Touchpad".to_owned(), ..pop(off, &format!("couldn't change it: {error}")) },
        None => pop(off, ""),
    })
}

/// What the Bluetooth key shows: on or off, or why it couldn't be changed.
fn bluetooth_pop(on: bool, problem: Option<&str>) -> Pop {
    let icon = if on { "bluetooth-active-symbolic" } else { "bluetooth-disabled-symbolic" };
    match problem {
        Some(problem) => Pop { kind: "bluetooth", icon, title: "Bluetooth".to_owned(), body: format!("couldn't change it: {problem}"), percent: -1 },
        None => Pop { kind: "bluetooth", icon, title: if on { "Bluetooth on".to_owned() } else { "Bluetooth off".to_owned() }, body: String::new(), percent: -1 },
    }
}

/// The first Bluetooth adapter BlueZ knows, as `/org/bluez/hciN`, with whether it is powered.
fn bluetooth_adapter(connection: &zbus::Connection) -> Result<(String, bool), String> {
    async_io::block_on(async {
        for number in 0..4 {
            let path = format!("/org/bluez/hci{number}");
            let reply = connection.call_method(Some("org.bluez"), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &("org.bluez.Adapter1", "Powered")).await;
            match reply {
                Ok(reply) => {
                    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().map_err(|error| error.to_string())?;
                    return Ok((path, bool::try_from(value).map_err(|error| error.to_string())?));
                }
                Err(zbus::Error::MethodError(name, ..)) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => return Err("Bluetooth isn't running".to_owned()),
                Err(_) => continue,
            }
        }
        Err("this laptop has no Bluetooth adapter".to_owned())
    })
}

/// The Bluetooth key: powers the adapter off, or on again (`toggle`, `on` or `off`), through BlueZ (no root: whoever is at the machine may).
/// What the pop-up says is what BlueZ reports afterwards.
fn bluetooth(action: &str) -> Result<Pop, String> {
    let connection = async_io::block_on(zbus::Connection::system()).map_err(|error| format!("the system bus: {error}"))?;
    let (path, on_now) = match bluetooth_adapter(&connection) {
        Ok(found) => found,
        Err(problem) => return Ok(bluetooth_pop(false, Some(&problem))),
    };
    let want_on = match action {
        "" | "toggle" => !on_now,
        "on" => true,
        "off" => false,
        _ => return Err("usage: autark-osd bluetooth [toggle|on|off]".to_owned()),
    };
    let set = async_io::block_on(connection.call_method(Some("org.bluez"), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Set", &("org.bluez.Adapter1", "Powered", Value::from(want_on))));
    let problem = set.err().map(|error| match error {
        zbus::Error::MethodError(_, Some(message), _) => message,
        zbus::Error::FDO(error) => error.to_string(),
        error => error.to_string(),
    });
    let on = bluetooth_adapter(&connection).map_or(on_now, |(_, on)| on);
    Ok(bluetooth_pop(on, problem.as_deref()))
}

/// The projection key: `autark-displays project` goes to the next way of using the screens and says which. A refusal (no other screen is
/// connected) is shown too, so the key never looks dead.
fn screens() -> Result<Pop, String> {
    let here = std::env::current_exe().ok().and_then(|path| path.parent().map(|dir| dir.join("autark-displays")));
    let program = here.filter(|path| path.exists()).map_or_else(|| "autark-displays".into(), |path| path.into_os_string());
    let output = Command::new(program).arg("project").stdin(Stdio::null()).output().map_err(|error| format!("autark-displays: {error}"))?;
    let (title, body) = if output.status.success() {
        (String::from_utf8_lossy(&output.stdout).trim().to_owned(), String::new())
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        ("Screens".to_owned(), said.trim().trim_start_matches("displays: ").to_owned())
    };
    Ok(Pop { kind: "screens", icon: "video-display-symbolic", title, body, percent: -1 })
}

/// Where the id of this kind's last pop-up is kept, so the next replaces it instead of stacking.
fn id_file(kind: &str) -> std::path::PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    runtime.join(format!("autark-osd-{kind}.id"))
}

fn show(pop: &Pop) -> Result<(), String> {
    let previous: u32 = std::fs::read_to_string(id_file(pop.kind)).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("transient", Value::from(true));
    hints.insert("urgency", Value::from(0u8));
    if pop.percent >= 0 {
        hints.insert("value", Value::from(pop.percent));
    }
    let id: u32 = async_io::block_on(async {
        let connection = zbus::Connection::session().await.map_err(|error| format!("the session bus: {error}"))?;
        let reply = connection
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &("Autark", previous, pop.icon, pop.title.as_str(), pop.body.as_str(), Vec::<&str>::new(), hints, SHOWN_MS),
            )
            .await
            .map_err(|error| format!("the notification daemon: {error}"))?;
        reply.body().deserialize::<u32>().map_err(|error| error.to_string())
    })?;
    let _ = std::fs::write(id_file(pop.kind), id.to_string());
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let action = args.get(1).map_or("", String::as_str);
    let pop = match args.first().map(String::as_str) {
        Some("volume") => volume(action)?,
        Some("mic") => microphone(action)?,
        Some("brightness") => brightness(action)?,
        Some("screens") => screens()?,
        Some("touchpad") => touchpad(action)?,
        Some("bluetooth") => bluetooth(action)?,
        _ => return Err("usage: autark-osd volume up|down|mute | mic mute | brightness up|down | screens | touchpad|bluetooth [toggle|on|off]".to_owned()),
    };
    // The change is made; a missing pop-up shouldn't make the key look broken, so it's only said.
    if let Err(error) = show(&pop) {
        eprintln!("autark-osd: {error}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("autark-osd: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpctl_volume_is_read_with_its_mute_flag() {
        assert_eq!(parse_volume("Volume: 0.75\n"), Some((75, false)));
        assert_eq!(parse_volume("Volume: 0.75 [MUTED]\n"), Some((75, true)));
        assert_eq!(parse_volume("Volume: 1.00 [MUTED]"), Some((100, true)));
        assert_eq!(parse_volume("Volume: 0.05"), Some((5, false)));
        assert_eq!(parse_volume(""), None);
        assert_eq!(parse_volume("not volume"), None);
    }

    #[test]
    fn touchpads_are_found_by_name_with_their_state() {
        let root = std::env::temp_dir().join(format!("autark-osd-touchpads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let put = |path: &str, content: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
        };
        put("class/input/input3/name", "AT Translated Set 2 keyboard\n");
        put("class/input/input3/inhibited", "0\n");
        put("class/input/input11/name", "ELAN0307:00 04F3:3282 Touchpad\n");
        put("class/input/input11/inhibited", "1\n");
        put("class/input/input12/name", "Another TouchPad\n");
        put("class/input/input12/inhibited", "0\n");
        put("class/input/event11/name", "ELAN0307:00 04F3:3282 Touchpad\n");
        put("class/input/event11/inhibited", "0\n");
        put("class/input/input13/name", "Touchpad with no switch\n");
        assert_eq!(touchpads(&root), vec![("input11".to_owned(), true), ("input12".to_owned(), false)]);
        assert!(touchpads(&root.join("nowhere")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_bluetooth_pop_up_says_what_happened() {
        assert_eq!(bluetooth_pop(true, None).title, "Bluetooth on");
        assert_eq!(bluetooth_pop(false, None).title, "Bluetooth off");
        assert_eq!(bluetooth_pop(false, None).icon, "bluetooth-disabled-symbolic");
        let failed = bluetooth_pop(false, Some("Blocked through rfkill"));
        assert_eq!((failed.title.as_str(), failed.body.as_str()), ("Bluetooth", "couldn't change it: Blocked through rfkill"));
    }

    #[test]
    fn the_bar_fills_in_proportion() {
        assert_eq!(bar(0), "░".repeat(20));
        assert_eq!(bar(100), "█".repeat(20));
        assert_eq!(bar(50), "█".repeat(10) + &"░".repeat(10));
        assert_eq!(bar(150).chars().count(), 20);
        assert_eq!(bar(5).chars().filter(|c| *c == '█').count(), 1);
    }

    #[test]
    fn the_speaker_icon_follows_the_level_and_mute() {
        assert_eq!(speaker_icon(75, true), "audio-volume-muted-symbolic");
        assert_eq!(speaker_icon(0, false), "audio-volume-muted-symbolic");
        assert_eq!(speaker_icon(20, false), "audio-volume-low-symbolic");
        assert_eq!(speaker_icon(50, false), "audio-volume-medium-symbolic");
        assert_eq!(speaker_icon(90, false), "audio-volume-high-symbolic");
    }
}
