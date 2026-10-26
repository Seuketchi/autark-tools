//! The terminal's colours: Linen's surface and text for the background and foreground, and a muted warm set of sixteen for
//! what programs paint. Light and dark each have their own sixteen, tuned so every colour reads on its own background
//! (the "bright black" is a readable grey, not the invisible one many terminals ship).

use autark_common::theme::{Look, Palette};
use gtk::gdk;

use crate::config::{Config, Scheme};
use vte::{Terminal, TerminalExt, TerminalExtManual};

const LIGHT: [&str; 16] = [
    "#3A3731", "#B5483A", "#4F7A52", "#9A7420", "#3F6A98", "#8A5A8F", "#2F7F80", "#CFC9BE", // black … white
    "#6B665C", "#CB5A4A", "#5F8F62", "#B08A2C", "#527FAE", "#A06FA6", "#3D9496", "#F2EEE6", // bright
];

const DARK: [&str; 16] = [
    "#2E2B26", "#E0806F", "#8DBE8A", "#D8B263", "#7FA6D6", "#C092C6", "#6DBDBE", "#D8D2C6", // black … white
    "#7C766A", "#EE9A8A", "#A3D0A0", "#E6C77E", "#98BCE6", "#D3A8D8", "#88CFCF", "#F4F0E8", // bright
];

fn parse(hex: &str) -> Option<gdk::RGBA> {
    gdk::RGBA::parse(hex.trim()).ok()
}

fn rgba(hex: &str) -> gdk::RGBA {
    parse(hex).unwrap_or(gdk::RGBA::BLACK)
}

/// The palette the terminal uses, and whether it's dark: the desktop's, or the light or dark one `terminal.toml` asks for.
pub fn scheme(look: &Look, config: &Config) -> (Palette, bool) {
    match config.scheme {
        Scheme::Follow => (look.palette.clone(), look.dark),
        Scheme::Light | Scheme::Dark => {
            let dark = config.scheme == Scheme::Dark;
            (look.theme.palette_for(dark), dark)
        }
    }
}

/// The window's title bar, buttons, tabs and bars in `palette`, for a window marked `scheme-override`. The desktop's stylesheet gives the
/// window the desktop's colours; when the terminal's own scheme is the other one, these rules (as literal colours, since the
/// named ones are the desktop's) win over them.
pub fn chrome_css(p: &Palette) -> String {
    let w = "window.terminal-window.scheme-override";
    format!(
        "{w} {{ background-color: {sidebar}; color: {heading}; }}
{w} .terminal-titlebar {{ border-bottom-color: alpha({text}, 0.06); }}
{w} .terminal-titlebar .window-title {{ color: {heading}; }}
{w} windowcontrols button {{ color: {text2}; }}
{w} windowcontrols button:hover {{ background-color: alpha({text}, 0.08); color: {text}; }}
{w} windowcontrols button > image, {w} windowcontrols button:hover > image, {w} windowcontrols button:active > image {{ background: none; box-shadow: none; }}
{w} windowcontrols button.close:hover {{ background-color: {danger}; color: #FFFFFF; }}
{w} .tabs > header {{ background-color: {sidebar}; border-bottom-color: {border}; }}
{w} .tabs > header.top > tabs > tab {{ color: {text2}; }}
{w} .tabs > header.top > tabs > tab:hover {{ color: {text}; }}
{w} .tabs > header.top > tabs > tab:checked {{ background-color: {surface}; color: {text}; }}
{w} .tabs > header button.flat {{ color: {text3}; }}
{w} .tabs > header button.flat:hover {{ background-color: {accent_soft}; color: {text}; }}
{w} .paste-warning {{ background-color: {raised}; border-bottom-color: {border}; color: {text}; }}
{w} .paste-warning label {{ color: {danger}; }}
{w} searchbar {{ background-color: {sidebar}; border-bottom-color: {border}; }}
",
        sidebar = p.sidebar, heading = p.heading, text = p.text, text2 = p.text2, text3 = p.text3, danger = p.danger, border = p.border,
        surface = p.surface, accent_soft = p.accent_soft, raised = p.raised,
    )
}

/// Paints `terminal` as the design's Terminal window: the sidebar's warm linen behind the heading colour, a block cursor in the
/// text colour, the accent on the selection. `config` can force a light or dark body and replace any of the colours.
pub fn apply(terminal: &Terminal, look: &Look, config: &Config) {
    let (palette, dark) = scheme(look, config);
    let mut sixteen: Vec<gdk::RGBA> = (if dark { DARK } else { LIGHT }).iter().map(|hex| rgba(hex)).collect();
    if let Some(custom) = config.palette.as_ref().filter(|custom| custom.len() == 16) {
        let parsed: Vec<gdk::RGBA> = custom.iter().filter_map(|hex| parse(hex)).collect();
        if parsed.len() == 16 {
            sixteen = parsed;
        }
    }
    let sixteen: Vec<&gdk::RGBA> = sixteen.iter().collect();
    let pick = |custom: &Option<String>, fallback: &str| custom.as_deref().and_then(parse).unwrap_or_else(|| rgba(fallback));
    let foreground = pick(&config.foreground, &palette.heading);
    let background = pick(&config.background, &palette.sidebar);
    terminal.set_colors(Some(&foreground), Some(&background), &sixteen);
    terminal.set_color_cursor(Some(&pick(&config.cursor_color, &palette.text)));
    terminal.set_color_cursor_foreground(Some(&background));
    terminal.set_color_highlight(Some(&rgba(&palette.accent_soft)));
    terminal.set_color_highlight_foreground(Some(&rgba(&palette.text)));
}
