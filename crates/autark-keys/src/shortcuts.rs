//! The shortcuts Wayfire is set to answer, read from `wayfire.ini` and named for people. Nothing is listed that
//! isn't bound, and a binding changed in Settings or by hand shows as it now is.

use std::path::PathBuf;

/// One binding as written in the file.
#[derive(Debug, PartialEq, Eq)]
pub struct Binding {
    pub section: String,
    pub key: String,
    /// The keys, as written: `<super> KEY_E | <alt> KEY_F4`.
    pub keys: String,
    /// For a `[command]` binding, what it runs.
    pub command: Option<String>,
}

pub struct Group {
    pub name: &'static str,
    pub rows: Vec<(String, Vec<String>)>,
}

pub fn path() -> PathBuf {
    std::env::var_os("WAYFIRE_CONFIG_FILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/wayfire.ini"))
}

fn is_binding(value: &str) -> bool {
    value != "none" && (value.contains("KEY_") || value.contains("BTN_") || value.starts_with('<'))
}

/// Every binding in `text`: the `[command]` ones paired with their commands, and any other option whose value is keys.
pub fn parse(text: &str) -> Vec<Binding> {
    let mut section = String::new();
    let mut options: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = name.to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            options.push((section.clone(), key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let mut bindings = Vec::new();
    for (section, key, value) in &options {
        if section == "command" {
            let name = ["binding_", "repeatable_binding_", "always_binding_"].iter().find_map(|prefix| key.strip_prefix(prefix));
            let Some(name) = name else { continue };
            let command = options.iter().find(|(other, other_key, _)| other == "command" && other_key.strip_prefix("command_") == Some(name)).map(|(_, _, command)| command.clone());
            if let (true, Some(command)) = (is_binding(value), command) {
                bindings.push(Binding { section: section.clone(), key: key.clone(), keys: value.clone(), command: Some(command) });
            }
        } else if is_binding(value) {
            bindings.push(Binding { section: section.clone(), key: key.clone(), keys: value.clone(), command: None });
        }
    }
    bindings
}

/// How a binding is recognised.
enum Is {
    /// A `[command]` binding whose command is exactly this.
    Command(&'static str),
    /// A `[command]` binding whose command contains this.
    Runs(&'static str),
    /// A named option in a plugin's section.
    Setting(&'static str, &'static str),
}

use Is::{Command, Runs, Setting as Opt};

const GROUPS: [&str; 6] = ["Apps", "Windows", "Workspaces", "Screen", "Sound and display", "System"];

/// What each known binding is for, and where it's listed. The first that fits wins, so the narrow come before the wide.
const KNOWN: &[(&str, &str, Is)] = &[
    ("Apps", "Launcher", Command("autark-launcher")),
    ("Apps", "All apps", Runs("autark-launcher --apps")),
    ("Apps", "Ask Kith", Runs("autark-launcher --assistant")),
    ("Apps", "Clipboard history", Runs("autark-launcher --clipboard")),
    ("Apps", "Terminal", Command("foot")),
    ("Apps", "Files", Runs("autark-files")),
    ("Apps", "Notes", Runs("autark-notes")),
    ("Apps", "Calculator", Runs("autark-calc")),
    ("Apps", "Settings", Runs("autark-settings")),
    ("Apps", "Clock", Runs("autark-clock")),
    ("Apps", "Keyboard shortcuts", Runs("autark-keys")),
    ("Apps", "Force quit", Runs("autark-monitor --force-quit")),
    ("Apps", "Activity", Command("autark-monitor")),
    ("Windows", "Close window", Opt("core", "close_top_view")),
    ("Windows", "Full screen", Opt("wm-actions", "toggle_fullscreen")),
    ("Windows", "Maximise", Opt("wm-actions", "toggle_maximize")),
    ("Windows", "Minimise", Opt("wm-actions", "minimize")),
    ("Windows", "Keep on top", Opt("wm-actions", "toggle_always_on_top")),
    ("Windows", "Show the desktop", Opt("wm-actions", "toggle_showdesktop")),
    ("Windows", "Next window", Opt("switcher", "next_view")),
    ("Windows", "Previous window", Opt("switcher", "prev_view")),
    ("Windows", "Quick window switch", Opt("fast-switcher", "activate")),
    ("Windows", "Tile left", Opt("grid", "slot_l")),
    ("Windows", "Tile right", Opt("grid", "slot_r")),
    ("Windows", "Centre the window", Opt("grid", "slot_c")),
    ("Windows", "Restore its size", Opt("grid", "restore")),
    ("Windows", "Move a window (drag)", Opt("move", "activate")),
    ("Windows", "Resize a window (drag)", Opt("resize", "activate")),
    ("Workspaces", "Overview of workspaces", Opt("expo", "toggle")),
    ("Workspaces", "Workspace left", Opt("vswitch", "binding_left")),
    ("Workspaces", "Workspace right", Opt("vswitch", "binding_right")),
    ("Workspaces", "Workspace up", Opt("vswitch", "binding_up")),
    ("Workspaces", "Workspace down", Opt("vswitch", "binding_down")),
    ("Workspaces", "Take the window left", Opt("vswitch", "with_win_left")),
    ("Workspaces", "Take the window right", Opt("vswitch", "with_win_right")),
    ("Workspaces", "Take the window up", Opt("vswitch", "with_win_up")),
    ("Workspaces", "Take the window down", Opt("vswitch", "with_win_down")),
    ("Workspaces", "Next display", Opt("oswitch", "next_output")),
    ("Workspaces", "Take the window to the next display", Opt("oswitch", "next_output_with_win")),
    ("Screen", "Screenshot: whole screen", Runs("autark-shot screen")),
    ("Screen", "Screenshot: an area", Runs("autark-shot area")),
    ("Screen", "Screenshot options", Runs("autark-shot options")),
    ("Screen", "Record an area", Runs("autark-shot record area")),
    ("Screen", "Record the screen", Runs("autark-shot record")),
    ("Screen", "Invert colours", Opt("invert", "toggle")),
    ("Sound and display", "Volume up", Runs("set-volume -l 1.0 @DEFAULT_AUDIO_SINK@")),
    ("Sound and display", "Volume down", Runs("set-volume @DEFAULT_AUDIO_SINK@ 5%-")),
    ("Sound and display", "Mute", Runs("set-mute @DEFAULT_AUDIO_SINK@")),
    ("Sound and display", "Mute the microphone", Runs("set-mute @DEFAULT_AUDIO_SOURCE@")),
    ("Sound and display", "Project to another screen", Runs("autark-osd screens")),
    ("Sound and display", "Touchpad on or off", Runs("autark-osd touchpad")),
    ("Sound and display", "Brighter", Runs("light -A")),
    ("Sound and display", "Dimmer", Runs("light -U")),
    ("System", "Lock the screen", Runs("autark-lock")),
    ("System", "Log out", Runs("wlogout")),
    ("System", "Notification centre", Runs("org.autark.Notifications1")),
];

fn label_for(binding: &Binding) -> Option<(&'static str, &'static str)> {
    KNOWN.iter().find_map(|(group, label, is)| {
        let fits = match (is, &binding.command) {
            (Command(exact), Some(command)) => command == exact,
            (Runs(part), Some(command)) => command.contains(part),
            (Is::Setting(section, key), None) => binding.section == *section && binding.key == *key,
            _ => false,
        };
        fits.then_some((*group, *label))
    })
}

/// `<super> <shift> KEY_3` as `["Super", "Shift", "3"]`.
pub fn caps(combo: &str) -> Vec<String> {
    combo
        .split_whitespace()
        .map(|part| match part {
            "<super>" => "Super".to_owned(),
            "<ctrl>" => "Ctrl".to_owned(),
            "<alt>" => "Alt".to_owned(),
            "<shift>" => "Shift".to_owned(),
            other => other.strip_prefix("KEY_").or_else(|| other.strip_prefix("BTN_")).map_or_else(|| other.to_owned(), |name| key_name(name, other.starts_with("BTN_"))),
        })
        .collect()
}

fn key_name(name: &str, button: bool) -> String {
    if button {
        return format!("Mouse {}", name.to_lowercase());
    }
    match name {
        "SPACE" => "Space",
        "ENTER" => "Enter",
        "ESC" => "Esc",
        "TAB" => "Tab",
        "LEFT" => "←",
        "RIGHT" => "→",
        "UP" => "↑",
        "DOWN" => "↓",
        "COMMA" => ",",
        "DOT" => ".",
        "SLASH" => "/",
        "SYSRQ" => "Print",
        "MUTE" => "Mute key",
        "MICMUTE" => "Mic mute key",
        "VOLUMEUP" => "Volume up key",
        "VOLUMEDOWN" => "Volume down key",
        "BRIGHTNESSUP" => "Brightness up key",
        "BRIGHTNESSDOWN" => "Brightness down key",
        "BACKSPACE" => "Backspace",
        other if other.starts_with("KP") && other.len() > 2 => return format!("Num {}", &other[2..]),
        other => return other.to_owned(),
    }
    .to_owned()
}

/// A binding's alternatives, each as its keys: `a | b` is two.
fn alternatives(keys: &str) -> Vec<String> {
    keys.split('|').map(|combo| combo.trim().to_owned()).filter(|combo| !combo.is_empty()).collect()
}

/// The shortcuts in `text`, grouped and named; one row for each thing, however many ways to do it.
pub fn groups(text: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = GROUPS.iter().map(|name| Group { name, rows: Vec::new() }).collect();
    let mut other: Vec<(String, Vec<String>)> = Vec::new();
    for binding in parse(text) {
        let combos = alternatives(&binding.keys);
        match label_for(&binding) {
            Some((group, label)) => {
                let target = groups.iter_mut().find(|candidate| candidate.name == group).map(|candidate| &mut candidate.rows);
                if let Some(rows) = target {
                    match rows.iter_mut().find(|(known, _)| known == label) {
                        Some((_, existing)) => existing.extend(combos),
                        None => rows.push((label.to_owned(), combos)),
                    }
                }
            }
            None => {
                if let Some(command) = &binding.command {
                    other.push((command.split_whitespace().take(3).collect::<Vec<_>>().join(" "), combos));
                }
            }
        }
    }
    // Rows follow the order of the table, not of the file.
    for group in &mut groups {
        group.rows.sort_by_key(|(label, _)| KNOWN.iter().position(|(_, known, _)| known == label).unwrap_or(usize::MAX));
    }
    if !other.is_empty() {
        groups.push(Group { name: "Other", rows: other });
    }
    groups.retain(|group| !group.rows.is_empty());
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    const INI: &str = "\
[core]
close_top_view = <super> KEY_Q | <alt> KEY_F4
plugins = a \\
  b

[command]
binding_launcher = <super> KEY_SPACE
command_launcher = autark-launcher
binding_autark_super = <super>
command_autark_super = autark-launcher
repeatable_binding_volume_up = KEY_VOLUMEUP
command_volume_up = wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+
binding_mine = <ctrl> KEY_9
command_mine = my-script --now please
binding_unset = none
command_unset = ignored

[wayfire-shell]
toggle_menu = none

[expo]
toggle = <super> KEY_TAB
";

    #[test]
    fn bindings_pair_with_their_commands_and_skip_what_is_off() {
        let bindings = parse(INI);
        assert!(bindings.iter().any(|b| b.key == "binding_launcher" && b.command.as_deref() == Some("autark-launcher")));
        assert!(bindings.iter().any(|b| b.section == "expo" && b.keys == "<super> KEY_TAB" && b.command.is_none()));
        assert!(!bindings.iter().any(|b| b.key == "toggle_menu" || b.key == "binding_unset" || b.key == "plugins"));
    }

    #[test]
    fn two_ways_to_one_thing_make_one_row() {
        let groups = groups(INI);
        let apps = groups.iter().find(|group| group.name == "Apps").unwrap();
        let launcher = apps.rows.iter().find(|(label, _)| label == "Launcher").unwrap();
        assert_eq!(launcher.1, ["<super> KEY_SPACE", "<super>"]);
        let windows = groups.iter().find(|group| group.name == "Windows").unwrap();
        assert_eq!(windows.rows[0], ("Close window".to_owned(), vec!["<super> KEY_Q".to_owned(), "<alt> KEY_F4".to_owned()]));
        let other = groups.iter().find(|group| group.name == "Other").unwrap();
        assert_eq!(other.rows[0].0, "my-script --now please");
        assert!(groups.iter().any(|group| group.name == "Sound and display"));
    }

    #[test]
    fn keys_read_as_keycaps() {
        assert_eq!(caps("<super> <shift> KEY_3"), ["Super", "Shift", "3"]);
        assert_eq!(caps("<super>"), ["Super"]);
        assert_eq!(caps("<ctrl> <super> KEY_LEFT"), ["Ctrl", "Super", "←"]);
        assert_eq!(caps("<super> BTN_LEFT"), ["Super", "Mouse left"]);
        assert_eq!(caps("<super> KEY_KP7"), ["Super", "Num 7"]);
        assert_eq!(caps("KEY_SYSRQ"), ["Print"]);
    }
}
