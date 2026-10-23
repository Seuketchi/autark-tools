//! The shortcuts Wayfire is set to answer, read from `wayfire.ini` and named for people. Nothing is listed that
//! isn't bound, and a binding changed in Settings or by hand shows as it now is. Changing one is [`rebind`]: the edits that
//! give a row new keys, which [`crate::ini::update`] writes and Wayfire picks up on its own.

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
    pub rows: Vec<Row>,
}

/// One thing a shortcut does, however many ways there are to do it.
#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    /// Each way to do it, as keys: `<super> KEY_Q`.
    pub combos: Vec<String>,
    /// Where those are written in the file.
    pub bindings: Vec<Binding>,
}

pub fn path() -> PathBuf {
    crate::ini::path()
}

/// The shipped `wayfire.ini`, which a changed shortcut can go back to.
pub const DEFAULTS: &str = "/usr/share/wayfire/wayfire.ini";

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
    ("Windows", "Next app", Command("autark-switcher next")),
    ("Windows", "Previous app", Command("autark-switcher prev")),
    ("Windows", "Next window of this app", Command("autark-switcher next-window")),
    ("Windows", "Previous window of this app", Command("autark-switcher prev-window")),
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
    ("Sound and display", "Bluetooth on or off", Runs("autark-osd bluetooth")),
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
            "<super>" | "<logo>" => "Super".to_owned(),
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
        "MINUS" => "-",
        "EQUAL" => "=",
        "SEMICOLON" => ";",
        "APOSTROPHE" => "'",
        "GRAVE" => "`",
        "BACKSLASH" => "\\",
        "LEFTBRACE" => "[",
        "RIGHTBRACE" => "]",
        "DELETE" => "Delete",
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

/// The shortcuts in `text`, grouped and named; one row for each thing, however many ways there are to do it.
pub fn groups(text: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = GROUPS.iter().map(|name| Group { name, rows: Vec::new() }).collect();
    let mut other: Vec<Row> = Vec::new();
    for binding in parse(text) {
        let combos = alternatives(&binding.keys);
        match label_for(&binding) {
            Some((group, label)) => {
                let target = groups.iter_mut().find(|candidate| candidate.name == group).map(|candidate| &mut candidate.rows);
                if let Some(rows) = target {
                    match rows.iter_mut().find(|known| known.label == label) {
                        Some(existing) => {
                            existing.combos.extend(combos);
                            existing.bindings.push(binding);
                        }
                        None => rows.push(Row { label: label.to_owned(), combos, bindings: vec![binding] }),
                    }
                }
            }
            None => {
                if let Some(command) = &binding.command {
                    other.push(Row { label: command.split_whitespace().take(3).collect::<Vec<_>>().join(" "), combos, bindings: vec![binding] });
                }
            }
        }
    }
    // Rows follow the order of the table, not of the file.
    for group in &mut groups {
        group.rows.sort_by_key(|row| KNOWN.iter().position(|(_, known, _)| *known == row.label).unwrap_or(usize::MAX));
    }
    if !other.is_empty() {
        groups.push(Group { name: "Other", rows: other });
    }
    groups.retain(|group| !group.rows.is_empty());
    groups
}

/// Keys written another way are still the same keys: `<shift> <super> KEY_A` and `<super> <shift> KEY_a`.
pub fn normalize(combo: &str) -> String {
    let mut parts: Vec<String> = combo.split_whitespace().map(|part| part.to_ascii_uppercase().replace("<LOGO>", "<SUPER>")).collect();
    parts.sort_by_key(|part| (!part.starts_with('<'), part.clone()));
    parts.join(" ")
}

/// The row that already answers `combo`, other than the one called `except`.
pub fn taken_by<'a>(groups: &'a [Group], combo: &str, except: &str) -> Option<&'a Row> {
    let wanted = normalize(combo);
    groups.iter().flat_map(|group| &group.rows).find(|row| row.label != except && row.combos.iter().any(|existing| normalize(existing) == wanted))
}

/// The option edits (`section/name`, and the value, which is `none` for a binding turned off) that give `row` the keys
/// `combo` and nothing else. The row's first binding takes them and any other binding of it is turned off.
pub fn rebind(row: &Row, combo: &str) -> Vec<(String, String)> {
    row.bindings
        .iter()
        .enumerate()
        .map(|(index, binding)| (format!("{}/{}", binding.section, binding.key), if index == 0 { combo.to_owned() } else { "none".to_owned() }))
        .collect()
}

/// The edits that put the row called `label` back as `defaults` (the shipped `wayfire.ini`) has it: every binding it has there,
/// as it is there. Empty when the defaults don't have that row.
pub fn reset(label: &str, defaults: &str) -> Vec<(String, String)> {
    let Some(row) = groups(defaults).into_iter().flat_map(|group| group.rows).find(|row| row.label == label) else { return Vec::new() };
    row.bindings.into_iter().map(|binding| (format!("{}/{}", binding.section, binding.key), binding.keys)).collect()
}

/// Whether `row` has the keys the shipped file gives it. A row the defaults lack, one you added, counts as its own default.
pub fn is_default(row: &Row, defaults: &str) -> bool {
    let sorted = |combos: &[String]| {
        let mut all: Vec<String> = combos.iter().map(|combo| normalize(combo)).collect();
        all.sort();
        all
    };
    match groups(defaults).into_iter().flat_map(|group| group.rows).find(|candidate| candidate.label == row.label) {
        Some(default) => sorted(&default.combos) == sorted(&row.combos),
        None => true,
    }
}

/// Applies `edits` to the file at `path`.
pub fn write(path: &std::path::Path, edits: &[(String, String)]) -> Result<(), String> {
    let options: Vec<(&str, Option<&str>)> = edits.iter().map(|(option, value)| (option.as_str(), Some(value.as_str()))).collect();
    crate::ini::update(path, &options)
}

/// The Linux key name for an evdev code, for the keys a shortcut can use.
pub fn evdev_name(code: u32) -> Option<&'static str> {
    const ROW: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "MINUS", "EQUAL"];
    const TOP: [&str; 12] = ["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "LEFTBRACE", "RIGHTBRACE"];
    const HOME: [&str; 12] = ["A", "S", "D", "F", "G", "H", "J", "K", "L", "SEMICOLON", "APOSTROPHE", "GRAVE"];
    const BOTTOM: [&str; 10] = ["Z", "X", "C", "V", "B", "N", "M", "COMMA", "DOT", "SLASH"];
    const FUNCTION: [&str; 10] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10"];
    Some(match code {
        1 => "ESC",
        2..=13 => ROW[(code - 2) as usize],
        14 => "BACKSPACE",
        15 => "TAB",
        16..=27 => TOP[(code - 16) as usize],
        28 => "ENTER",
        30..=41 => HOME[(code - 30) as usize],
        43 => "BACKSLASH",
        44..=53 => BOTTOM[(code - 44) as usize],
        57 => "SPACE",
        59..=68 => FUNCTION[(code - 59) as usize],
        87 => "F11",
        88 => "F12",
        99 => "SYSRQ",
        102 => "HOME",
        103 => "UP",
        104 => "PAGEUP",
        105 => "LEFT",
        106 => "RIGHT",
        107 => "END",
        108 => "DOWN",
        109 => "PAGEDOWN",
        110 => "INSERT",
        111 => "DELETE",
        113 => "MUTE",
        114 => "VOLUMEDOWN",
        115 => "VOLUMEUP",
        224 => "BRIGHTNESSDOWN",
        225 => "BRIGHTNESSUP",
        248 => "MICMUTE",
        _ => return None,
    })
}

/// A key press as a Wayfire binding, if it can be one: a modifier and a key, or a function or media key alone. A typing key
/// alone, or with only Shift, would stop typing, so it isn't.
pub fn combo(code: u32, ctrl: bool, alt: bool, shift: bool, logo: bool) -> Option<String> {
    let key = evdev_name(code)?;
    let mut parts: Vec<&str> = [(ctrl, "<ctrl>"), (alt, "<alt>"), (shift, "<shift>"), (logo, "<super>")].into_iter().filter(|(on, _)| *on).map(|(_, name)| name).collect();
    let standalone = (key.starts_with('F') && key.len() > 1 && key[1..].parse::<u8>().is_ok()) || matches!(key, "SYSRQ" | "MUTE" | "VOLUMEDOWN" | "VOLUMEUP" | "BRIGHTNESSDOWN" | "BRIGHTNESSUP" | "MICMUTE");
    if (parts.is_empty() || parts == ["<shift>"]) && !standalone {
        return None;
    }
    let key = format!("KEY_{key}");
    parts.push(&key);
    Some(parts.join(" "))
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
        let launcher = apps.rows.iter().find(|row| row.label == "Launcher").unwrap();
        assert_eq!(launcher.combos, ["<super> KEY_SPACE", "<super>"]);
        assert_eq!(launcher.bindings.iter().map(|binding| binding.key.as_str()).collect::<Vec<_>>(), ["binding_launcher", "binding_autark_super"]);
        let windows = groups.iter().find(|group| group.name == "Windows").unwrap();
        assert_eq!((windows.rows[0].label.as_str(), &windows.rows[0].combos[..]), ("Close window", &["<super> KEY_Q".to_owned(), "<alt> KEY_F4".to_owned()][..]));
        let other = groups.iter().find(|group| group.name == "Other").unwrap();
        assert_eq!(other.rows[0].label, "my-script --now please");
        assert!(groups.iter().any(|group| group.name == "Sound and display"));
    }

    #[test]
    fn the_app_switcher_is_named() {
        let ini = "[command]\nbinding_switch_next = <alt> KEY_TAB\ncommand_switch_next = autark-switcher next\nbinding_switch_window_prev = <alt> <shift> KEY_GRAVE\ncommand_switch_window_prev = autark-switcher prev-window\n";
        let groups = groups(ini);
        let windows = groups.iter().find(|group| group.name == "Windows").unwrap();
        let row = |label: &str| windows.rows.iter().find(|row| row.label == label).unwrap();
        assert_eq!(row("Next app").combos, ["<alt> KEY_TAB"]);
        assert_eq!(row("Previous window of this app").combos, ["<alt> <shift> KEY_GRAVE"]);
        assert!(groups.iter().all(|group| group.name != "Other"));
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

    const DEFAULTS_INI: &str = "\
[core]
close_top_view = <super> KEY_Q | <alt> KEY_F4

[command]
binding_launcher = <super> KEY_SPACE
command_launcher = autark-launcher
binding_autark_super = <super>
command_autark_super = autark-launcher

[expo]
toggle = <super> KEY_TAB
";

    fn row<'a>(groups: &'a [Group], label: &str) -> &'a Row {
        groups.iter().flat_map(|group| &group.rows).find(|row| row.label == label).unwrap()
    }

    #[test]
    fn a_combination_is_the_same_however_it_is_written() {
        assert_eq!(normalize("<shift> <super> KEY_A"), normalize("<super> <shift> key_a"));
        assert_eq!(normalize("<logo> KEY_E"), normalize("<super> KEY_E"));
        assert_ne!(normalize("<super> KEY_E"), normalize("<ctrl> KEY_E"));
    }

    #[test]
    fn a_combination_in_use_names_who_has_it() {
        let groups = groups(DEFAULTS_INI);
        assert_eq!(taken_by(&groups, "<super> <alt> KEY_Q", "Overview of workspaces").map(|row| row.label.as_str()), None);
        assert_eq!(taken_by(&groups, "<alt> KEY_F4", "Overview of workspaces").map(|row| row.label.as_str()), Some("Close window"));
        assert_eq!(taken_by(&groups, "<super> KEY_TAB", "Overview of workspaces"), None, "a row doesn't conflict with itself");
        assert_eq!(taken_by(&groups, "<logo>", "Close window").map(|row| row.label.as_str()), Some("Launcher"));
    }

    #[test]
    fn rebinding_gives_a_row_one_combination() {
        let groups = groups(DEFAULTS_INI);
        assert_eq!(rebind(row(&groups, "Close window"), "<ctrl> KEY_W"), [("core/close_top_view".to_owned(), "<ctrl> KEY_W".to_owned())]);
        // The launcher is two bindings; the second is turned off rather than left to answer.
        assert_eq!(
            rebind(row(&groups, "Launcher"), "<ctrl> KEY_SPACE"),
            [("command/binding_launcher".to_owned(), "<ctrl> KEY_SPACE".to_owned()), ("command/binding_autark_super".to_owned(), "none".to_owned())]
        );
    }

    #[test]
    fn a_rebound_row_can_go_back_to_the_shipped_keys() {
        let dir = std::env::temp_dir().join(format!("autark-shortcuts-{}", std::process::id()));
        let file = dir.join("wayfire.ini");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, DEFAULTS_INI).unwrap();
        let current = groups(&std::fs::read_to_string(&file).unwrap());
        assert!(is_default(row(&current, "Launcher"), DEFAULTS_INI));
        write(&file, &rebind(row(&current, "Launcher"), "<ctrl> KEY_SPACE")).unwrap();
        let changed = std::fs::read_to_string(&file).unwrap();
        let after = groups(&changed);
        let launcher = row(&after, "Launcher");
        assert_eq!(launcher.combos, ["<ctrl> KEY_SPACE"], "the second way is off");
        assert!(!is_default(launcher, DEFAULTS_INI));
        assert!(is_default(row(&after, "Close window"), DEFAULTS_INI), "the rest is as it was");
        write(&file, &reset("Launcher", DEFAULTS_INI)).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), DEFAULTS_INI);
        assert!(reset("Nonsense", DEFAULTS_INI).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_key_press_is_a_binding_only_when_it_can_be() {
        assert_eq!(combo(57, false, false, false, true).as_deref(), Some("<super> KEY_SPACE"));
        assert_eq!(combo(4, true, false, true, true).as_deref(), Some("<ctrl> <shift> <super> KEY_3"));
        assert_eq!(combo(30, false, false, false, false), None, "a plain letter would stop typing");
        assert_eq!(combo(30, false, false, true, false), None);
        assert_eq!(combo(88, false, false, false, false).as_deref(), Some("KEY_F12"));
        assert_eq!(combo(115, false, false, false, false).as_deref(), Some("KEY_VOLUMEUP"));
        assert_eq!(combo(250, false, false, false, true), None, "a key that has no name here");
        assert_eq!(caps("<shift> <super> KEY_3"), ["Shift", "Super", "3"]);
    }
}
