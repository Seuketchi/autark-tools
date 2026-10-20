//! `autark-osd volume up|down|mute`, `autark-osd mic mute`, `autark-osd brightness up|down`, `autark-osd screens`: what the volume, mute,
//! microphone, brightness and projection keys run. It makes the change and shows the result as a pop-up, so a press is visible: a transient notification (never kept in
//! the notification centre) that replaces the last one of its kind, with a bar, through `org.freedesktop.Notifications`.

use std::collections::HashMap;
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
        _ => return Err("usage: autark-osd volume up|down|mute | mic mute | brightness up|down | screens".to_owned()),
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
