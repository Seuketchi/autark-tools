//! The terminal's colours: Linen's surface and text for the background and foreground, and a muted warm set of sixteen for
//! what programs paint. Light and dark each have their own sixteen, tuned so every colour reads on its own background
//! (the "bright black" is a readable grey, not the invisible one many terminals ship).

use autark_common::theme::Palette;
use gtk::gdk;
use vte::{Terminal, TerminalExt, TerminalExtManual};

const LIGHT: [&str; 16] = [
    "#3A3731", "#B5483A", "#4F7A52", "#9A7420", "#3F6A98", "#8A5A8F", "#2F7F80", "#CFC9BE", // black … white
    "#6B665C", "#CB5A4A", "#5F8F62", "#B08A2C", "#527FAE", "#A06FA6", "#3D9496", "#F2EEE6", // bright
];

const DARK: [&str; 16] = [
    "#2E2B26", "#E0806F", "#8DBE8A", "#D8B263", "#7FA6D6", "#C092C6", "#6DBDBE", "#D8D2C6", // black … white
    "#7C766A", "#EE9A8A", "#A3D0A0", "#E6C77E", "#98BCE6", "#D3A8D8", "#88CFCF", "#F4F0E8", // bright
];

fn rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::BLACK)
}

/// Paints `terminal` with the palette: Linen's surface behind Linen's text, the accent on the cursor and selection.
pub fn apply(terminal: &Terminal, palette: &Palette, dark: bool) {
    let sixteen: Vec<gdk::RGBA> = (if dark { DARK } else { LIGHT }).iter().map(|hex| rgba(hex)).collect();
    let sixteen: Vec<&gdk::RGBA> = sixteen.iter().collect();
    terminal.set_colors(Some(&rgba(&palette.text)), Some(&rgba(&palette.surface)), &sixteen);
    terminal.set_color_cursor(Some(&rgba(&palette.accent)));
    terminal.set_color_cursor_foreground(Some(&rgba(&palette.on_accent)));
    terminal.set_color_highlight(Some(&rgba(&palette.accent_soft)));
    terminal.set_color_highlight_foreground(Some(&rgba(&palette.text)));
}
