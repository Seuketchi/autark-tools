//! The panes, in the sidebar's order, and the names search finds.

mod appearance;
mod display;
mod dock;
mod night;
mod sound;

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

pub const PANES: [Pane; 11] = [
    Pane { id: "wifi", label: "Wi-Fi", glyph: "M2 8.5a15 15 0 0 1 20 0M5 12a10 10 0 0 1 14 0M8.5 15.5a5 5 0 0 1 7 0M12 19h.01", tile: BLUE, gap: false },
    Pane { id: "bluetooth", label: "Bluetooth", glyph: "M7 7l10 10l-5 4V3l5 4L7 17", tile: BLUE, gap: false },
    Pane { id: "appearance", label: "Appearance", glyph: "M12 3a9 9 0 1 0 0 18zM12 3a9 9 0 0 1 0 18", tile: GRAPHITE, gap: true },
    Pane { id: "dock", label: "Desktop & Dock", glyph: "M3 5h18v11H3zM8 20h8M7 16v4M17 16v4", tile: GRAPHITE, gap: false },
    Pane { id: "display", label: "Display", glyph: "M3 4h18v12H3zM9 20h6M12 16v4", tile: ("#4FD8E6", "#1380C4"), gap: false },
    Pane { id: "sound", label: "Sound", glyph: "M4 9h4l5-4v14l-5-4H4zM16.5 8.5a5 5 0 0 1 0 7", tile: PINK, gap: false },
    Pane { id: "power", label: "Power", glyph: "M13 3L5 13h6l-1 8l8-10h-6z", tile: ("#5EDC94", "#1C9A57"), gap: true },
    Pane { id: "lock", label: "Fingerprint & Lock", glyph: "M6 11h12v10H6zM8.5 11V8a3.5 3.5 0 0 1 7 0v3", tile: PINK, gap: false },
    Pane { id: "keyboard", label: "Keyboard", glyph: "M3 6h18v12H3zM7 10h.01M11 10h.01M15 10h.01M7 14h10", tile: SILVER, gap: false },
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
    ("Key repeat", "keyboard"),
    ("Caps Lock", "keyboard"),
    ("Keyboard layouts", "keyboard"),
    ("Shortcuts", "keyboard"),
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
        "sound" => sound::build(),
        _ => {
            let (page, column) = ui::page();
            column.append(&ui::title(find(id).map_or(id, |pane| pane.label), None));
            ui::unavailable(&column, "This pane isn't built yet.");
            page.upcast()
        }
    }
}
