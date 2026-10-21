//! The panes, in the sidebar's order, and the names search finds.

mod about;
mod accessibility;
mod ai;
mod appearance;
mod bluetooth;
mod datetime;
mod defaults;
mod display;
mod dock;
mod keyboard;
mod language;
mod lock;
mod mouse;
mod network;
mod notifications;
mod night;
mod password;
mod power;
mod privacy;
mod sound;
mod storage;
mod terminal;
mod updates;
mod users;
mod wifi;
mod windows;

use gtk::prelude::*;

use crate::ui;

pub struct Pane {
    pub id: &'static str,
    pub label: &'static str,
    /// SVG path data on a 24-unit grid.
    pub glyph: &'static str,
    /// The tile's gradient, top to bottom.
    pub tile: (&'static str, &'static str),
    /// A little space above it in the sidebar.
    pub gap: bool,
}

const BLUE: (&str, &str) = ("#6AB0FF", "#2C6BE0");
const GRAPHITE: (&str, &str) = ("#454A55", "#141619");
const PINK: (&str, &str) = ("#FF7A96", "#D92B55");
const SILVER: (&str, &str) = ("#C3C9D2", "#6A717E");

pub const PANES: [Pane; 24] = [
    Pane { id: "wifi", label: "Wi-Fi", glyph: "M2 8.5a15 15 0 0 1 20 0M5 12a10 10 0 0 1 14 0M8.5 15.5a5 5 0 0 1 7 0M12 19h.01", tile: BLUE, gap: false },
    Pane { id: "network", label: "Network", glyph: "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18zM3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18", tile: BLUE, gap: false },
    Pane { id: "bluetooth", label: "Bluetooth", glyph: "M7 7l10 10l-5 4V3l5 4L7 17", tile: BLUE, gap: false },
    Pane { id: "appearance", label: "Appearance", glyph: "M12 3a9 9 0 1 0 0 18zM12 3a9 9 0 0 1 0 18", tile: GRAPHITE, gap: true },
    Pane { id: "dock", label: "Desktop & Dock", glyph: "M3 5h18v11H3zM8 20h8M7 16v4M17 16v4", tile: GRAPHITE, gap: false },
    Pane { id: "windows", label: "Windows", glyph: "M3 5h18v14H3zM3 9h18M6 7h.01M9 7h.01", tile: GRAPHITE, gap: false },
    Pane { id: "display", label: "Display", glyph: "M3 4h18v12H3zM9 20h6M12 16v4", tile: ("#4FD8E6", "#1380C4"), gap: false },
    Pane { id: "sound", label: "Sound", glyph: "M4 9h4l5-4v14l-5-4H4zM16.5 8.5a5 5 0 0 1 0 7", tile: PINK, gap: false },
    Pane { id: "notifications", label: "Notifications", glyph: "M6 9a6 6 0 0 1 12 0c0 6 2 7 2 7H4s2-1 2-7zM10 20a2 2 0 0 0 4 0", tile: ("#FF8F7A", "#D9402B"), gap: false },
    Pane { id: "datetime", label: "Date & Time", glyph: "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18zM12 7v5l3 2", tile: ("#FFBE5C", "#EE7418"), gap: true },
    Pane { id: "language", label: "Language & Region", glyph: "M3 5h12M9 3v2M5 5c1 5 4 8 8 10M13 5c-1 5-4 8-8 10M13 21l4-10l4 10M14.5 18h5", tile: ("#A88BFF", "#5537D2"), gap: false },
    Pane { id: "defaults", label: "Default Apps", glyph: "M5 5h6v6H5zM13 5h6v6h-6zM5 13h6v6H5zM15 13l2 2l4-4", tile: ("#5EDC94", "#1C9A57"), gap: false },
    Pane { id: "power", label: "Power", glyph: "M13 3L5 13h6l-1 8l8-10h-6z", tile: ("#5EDC94", "#1C9A57"), gap: true },
    Pane { id: "privacy", label: "Privacy & Security", glyph: "M12 3l8 3v6c0 5-3.5 8-8 9c-4.5-1-8-4-8-9V6zM9 12l2 2l4-4", tile: ("#5A606C", "#1E2127"), gap: false },
    Pane { id: "lock", label: "Fingerprint & Lock", glyph: "M6 11h12v10H6zM8.5 11V8a3.5 3.5 0 0 1 7 0v3", tile: PINK, gap: false },
    Pane { id: "accessibility", label: "Accessibility", glyph: "M12 4a1.5 1.5 0 1 0 0 3a1.5 1.5 0 1 0 0-3zM5 9l7 1l7-1M12 10v4l-3 6M12 14l3 6", tile: BLUE, gap: false },
    Pane { id: "mouse", label: "Mouse & Touchpad", glyph: "M7 3h10a3 3 0 0 1 3 3v12a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3V6a3 3 0 0 1 3-3zM12 3v7M4 10h16", tile: SILVER, gap: false },
    Pane { id: "keyboard", label: "Keyboard", glyph: "M3 6h18v12H3zM7 10h.01M11 10h.01M15 10h.01M7 14h10", tile: SILVER, gap: false },
    Pane { id: "terminal", label: "Terminal", glyph: "M3 5h18v14H3zM7 10l3 2-3 2M12 15h5", tile: GRAPHITE, gap: false },
    Pane { id: "storage", label: "Storage", glyph: "M4 7c0-2 4-3 8-3s8 1 8 3s-4 3-8 3s-8-1-8-3zM4 7v10c0 2 4 3 8 3s8-1 8-3V7M4 12c0 2 4 3 8 3s8-1 8-3", tile: SILVER, gap: true },
    Pane { id: "users", label: "Users & Accounts", glyph: "M12 4a4 4 0 1 0 0 8a4 4 0 1 0 0-8zM4 20c0-4 4-6 8-6s8 2 8 6", tile: SILVER, gap: false },
    Pane { id: "updates", label: "Software Updates", glyph: "M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5", tile: ("#5EDC94", "#1C9A57"), gap: false },
    Pane {
        id: "ai",
        label: "AI",
        glyph: "M12 3l1.8 4.7L18.5 9.5l-4.7 1.8L12 16l-1.8-4.7L5.5 9.5l4.7-1.8zM19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z",
        tile: ("#7FB89A", "#3F7A5F"),
        gap: true,
    },
    Pane { id: "about", label: "About Autark", glyph: "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18zM12 11v5M12 8h.01", tile: SILVER, gap: false },
];

/// What search finds by name: a setting and the pane it's on.
pub const INDEX: &[(&str, &str)] = &[
    ("Wi-Fi", "wifi"),
    ("Known networks", "wifi"),
    ("Join a hidden network", "wifi"),
    ("Ethernet", "network"),
    ("IP address", "network"),
    ("DNS", "network"),
    ("Bluetooth", "bluetooth"),
    ("Pair a device", "bluetooth"),
    ("Theme", "appearance"),
    ("Dark mode", "appearance"),
    ("Accent colour", "appearance"),
    ("Wallpaper", "appearance"),
    ("Text size", "appearance"),
    ("Font", "appearance"),
    ("Reduce motion", "appearance"),
    ("Increase contrast", "appearance"),
    ("Dock size", "dock"),
    ("Magnify icons", "dock"),
    ("Dock position", "dock"),
    ("Hide the dock", "dock"),
    ("Widgets on the desktop", "dock"),
    ("Arrange displays", "display"),
    ("Resolution", "display"),
    ("Refresh rate", "display"),
    ("Scale", "display"),
    ("Brightness", "display"),
    ("Rotation", "display"),
    ("Night light", "display"),
    ("Volume", "sound"),
    ("Output device", "sound"),
    ("Microphone", "sound"),
    ("Sound effects", "sound"),
    ("Battery", "power"),
    ("Battery health", "power"),
    ("Screen off", "power"),
    ("Sleep", "power"),
    ("Low battery warning", "power"),
    ("Fingerprints", "lock"),
    ("Lock screen", "lock"),
    ("Password", "lock"),
    ("Fingerprint for sudo", "lock"),
    ("Time zone", "datetime"),
    ("Set time automatically", "datetime"),
    ("12-hour clock", "datetime"),
    ("Language", "language"),
    ("Region", "language"),
    ("Default browser", "defaults"),
    ("Default apps", "defaults"),
    ("Default terminal", "defaults"),
    ("Do not disturb", "notifications"),
    ("Focus", "notifications"),
    ("Banners", "notifications"),
    ("Disk encryption", "privacy"),
    ("Secure Boot", "privacy"),
    ("Firewall", "privacy"),
    ("Text size", "accessibility"),
    ("Pointer size", "accessibility"),
    ("Zoom the screen", "accessibility"),
    ("Disk space", "storage"),
    ("Empty Trash", "storage"),
    ("Account type", "users"),
    ("Administrator", "users"),
    ("Security watch", "updates"),
    ("Kernel", "updates"),
    ("Window animations", "windows"),
    ("Window placement", "windows"),
    ("Title bar height", "windows"),
    ("Pointer speed", "mouse"),
    ("Natural scrolling", "mouse"),
    ("Tap to click", "mouse"),
    ("Left-handed", "mouse"),
    ("Touchpad", "mouse"),
    ("Key repeat", "keyboard"),
    ("Caps Lock", "keyboard"),
    ("Keyboard layouts", "keyboard"),
    ("Shortcuts", "keyboard"),
    ("Terminal font", "terminal"),
    ("Terminal colours", "terminal"),
    ("Terminal cursor", "terminal"),
    ("Scrollback", "terminal"),
    ("Local-only mode", "ai"),
    ("Jev", "ai"),
    ("Ollama model", "ai"),
    ("Tasks", "ai"),
    ("Memory", "ai"),
    ("Call log", "ai"),
    ("About Autark", "about"),
];

pub fn find(id: &str) -> Option<&'static Pane> {
    PANES.iter().find(|pane| pane.id == id)
}

/// A pane's page, built when it's first opened.
pub fn build(id: &str) -> gtk::Widget {
    match id {
        "appearance" => appearance::build(),
        "display" => display::build(),
        "dock" => dock::build(),
        "windows" => windows::build(),
        "sound" => sound::build(),
        "power" => power::build(),
        "lock" => lock::build(),
        "keyboard" => keyboard::build(),
        "mouse" => mouse::build(),
        "notifications" => notifications::build(),
        "privacy" => privacy::build(),
        "accessibility" => accessibility::build(),
        "storage" => storage::build(),
        "users" => users::build(),
        "updates" => updates::build(),
        "datetime" => datetime::build(),
        "language" => language::build(),
        "defaults" => defaults::build(),
        "terminal" => terminal::build(),
        "wifi" => wifi::build(),
        "network" => network::build(),
        "bluetooth" => bluetooth::build(),
        "ai" => ai::build(),
        "about" => about::build(),
        _ => {
            let (page, column) = ui::page();
            column.append(&ui::title(find(id).map_or(id, |pane| pane.label), None));
            ui::unavailable(&column, "This pane isn't built yet.");
            page.upcast()
        }
    }
}
