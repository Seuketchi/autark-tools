//! The look: `theme.toml` (light, dark or auto, the accent, wallpaper, text size, font, reduced motion and
//! more contrast) and the colour tokens it gives. Linen is taken from `design/canvas/Colors.dc.html` in the
//! Autark repo; its dark side is derived from the four dark tokens there. With the `style` feature,
//! [`install`] loads a tool's stylesheet with the tokens and restyles it live when the theme changes.
//!
//! ```toml
//! theme = "auto"        # "light", "dark", or "auto" (dark from sunset to sunrise, as night light works it out)
//! accent = "sage"       # sage, blue, teal, plum, coral or stone
//! wallpaper = "linen"   # linen, ridge, dusk, daylight, or a picture's path
//! text_size = 100       # percent: 100, 115 or 130
//! font = "Geist"        # Geist or Inter
//! reduce_motion = false
//! contrast = false
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Light,
    Dark,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub theme: Mode,
    /// The id of the look chosen in Settings (see `looks`), or empty: its colour tokens apply over Linen's.
    pub look: String,
    pub accent: String,
    pub wallpaper: String,
    pub text_size: u32,
    pub font: String,
    pub reduce_motion: bool,
    pub contrast: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self { theme: Mode::Light, look: String::new(), accent: "sage".into(), wallpaper: "linen".into(), text_size: 100, font: "Geist".into(), reduce_motion: false, contrast: false }
    }
}

/// The accents: name, light hex, dark hex, and the text on the dark one.
pub const ACCENTS: [(&str, &str, &str, &str); 6] = [
    ("sage", "#3F7A5F", "#7FC1A1", "#0F2219"),
    ("blue", "#3D63E8", "#7C9BFF", "#0B1533"),
    ("teal", "#12807A", "#4CC9BF", "#062220"),
    ("plum", "#7B5CD6", "#A993F0", "#1A1033"),
    ("coral", "#C4503C", "#F28B76", "#2B0E08"),
    ("stone", "#6B655C", "#B3ACA2", "#1A1916"),
];

impl Theme {
    pub fn load() -> Self {
        crate::config::load("theme")
    }

    /// Whether it's dark now: always, never, or at night.
    pub fn dark(&self) -> bool {
        match self.theme {
            Mode::Light => false,
            Mode::Dark => true,
            Mode::Auto => is_night(),
        }
    }

    pub fn palette(&self) -> Palette {
        self.palette_for(self.dark())
    }

    /// The palette in the light or dark theme: Linen with this accent and contrast, then the chosen look's tokens.
    pub fn palette_for(&self, dark: bool) -> Palette {
        let mut palette = Palette::linen(dark, &self.accent, self.contrast);
        if !self.look.is_empty()
            && let Some(look) = crate::looks::find(&self.look)
        {
            look.apply(&mut palette, dark);
        }
        palette
    }

    /// The font stack for CSS, the chosen font first.
    pub fn font_stack(&self) -> String {
        match self.font.as_str() {
            "Inter" => "\"Inter\", \"Geist\", sans-serif".into(),
            "Geist" | "" => "\"Geist\", \"Inter\", sans-serif".into(),
            other => format!("\"{other}\", \"Geist\", sans-serif"),
        }
    }

    pub fn text_scale(&self) -> f64 {
        f64::from(self.text_size.clamp(80, 200)) / 100.0
    }
}

/// `$XDG_RUNTIME_DIR/autark/night`: "1" from sunset to sunrise, written by the night light daemon for the
/// auto theme.
pub fn night_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    runtime.join("autark").join("night")
}

pub fn is_night() -> bool {
    std::fs::read_to_string(night_path()).is_ok_and(|text| text.trim() == "1")
}

/// GTK's own size for its default font, in points, at the default text size.
const GTK_FONT_POINTS: f64 = 11.0;

/// Passes the theme on to other GTK apps through `gtk-3.0` and `gtk-4.0`'s `settings.ini`: dark or not, the
/// font and its size. Other keys and lines in those files stay. GTK reads them as an app starts, so it's
/// apps opened afterwards that follow.
pub fn write_gtk_settings(theme: &Theme) -> std::io::Result<()> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    let font = if theme.font.is_empty() { "Geist" } else { &theme.font };
    let points = (GTK_FONT_POINTS * theme.text_scale()).round();
    let keys = [
        ("gtk-application-prefer-dark-theme", if theme.dark() { "1".to_owned() } else { "0".to_owned() }),
        ("gtk-font-name", format!("{font} {points}")),
        // Minimise, maximise and close at the right, as Autark's own apps have them.
        ("gtk-decoration-layout", ":minimize,maximize,close".to_owned()),
    ];
    for version in ["gtk-3.0", "gtk-4.0"] {
        let path = config.join(version).join("settings.ini");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = set_ini(&text, &keys);
        if updated != text {
            std::fs::create_dir_all(config.join(version))?;
            std::fs::write(&path, updated)?;
        }
    }
    // libadwaita apps (the browser) learn the system is dark from GNOME's colour-scheme setting, which they only read with the portal
    // off (ADW_DISABLE_PORTAL=1, set for the session and in the browser's launcher). Without it their pages stay light.
    let _ = std::process::Command::new("gsettings").args(["set", "org.gnome.desktop.interface", "color-scheme", if theme.dark() { "prefer-dark" } else { "default" }]).output();
    write_gtk_css(&config.join("gtk-4.0"), &theme.palette())
}

const GTK_CSS_MARK: &str = "/* Written by Autark (theme.rs)";

/// `gtk-4.0/gtk.css`: Linen for libadwaita apps (the browser, and anything else built on it), as the named colours it
/// reads and the same window buttons. It replaces only a file this wrote, found by its first line, so a `gtk.css` of
/// your own is left alone. Autark's own apps aren't affected: they don't read these names, and their buttons carry
/// `autark-controls`, which the button rules skip.
fn write_gtk_css(folder: &Path, palette: &Palette) -> std::io::Result<()> {
    let path = folder.join("gtk.css");
    if let Ok(existing) = std::fs::read_to_string(&path)
        && !existing.starts_with(GTK_CSS_MARK)
    {
        return Ok(());
    }
    let p = palette;
    let text = format!(
        "{GTK_CSS_MARK}: Linen for libadwaita apps. Edits here are overwritten when the theme changes; delete the first line to keep your own. */
@define-color window_bg_color {surface};
@define-color window_fg_color {text};
@define-color view_bg_color {surface};
@define-color view_fg_color {text};
@define-color headerbar_bg_color {sidebar};
@define-color headerbar_fg_color {text};
@define-color headerbar_backdrop_color {sidebar};
@define-color headerbar_border_color {border};
@define-color sidebar_bg_color {sidebar};
@define-color sidebar_fg_color {text};
@define-color sidebar_backdrop_color {sidebar};
@define-color card_bg_color {raised};
@define-color card_fg_color {text};
@define-color popover_bg_color {raised};
@define-color popover_fg_color {text};
@define-color dialog_bg_color {raised};
@define-color dialog_fg_color {text};
@define-color accent_bg_color {accent};
@define-color accent_fg_color {on_accent};
@define-color accent_color {accent_text};
@define-color destructive_bg_color {danger};
@define-color error_bg_color {danger};

windowcontrols:not(.autark-controls) button {{
  min-width: 28px;
  min-height: 28px;
  margin: 0 0 0 2px;
  padding: 0;
  border: none;
  border-radius: 8px;
  box-shadow: none;
  background: none;
  color: {text2};
}}

windowcontrols:not(.autark-controls) button:hover {{
  background-color: alpha({text}, 0.08);
  color: {text};
}}

/* GTK's own theme paints a dark circle on the icon of a hovered window button, which would cover the red. */
windowcontrols:not(.autark-controls) button > image,
windowcontrols:not(.autark-controls) button:hover > image,
windowcontrols:not(.autark-controls) button:active > image {{
  background: none;
  box-shadow: none;
}}

windowcontrols:not(.autark-controls) button.close:hover {{
  background-color: {danger};
  color: #FFFFFF;
}}
",
        surface = p.surface,
        text = p.text,
        text2 = p.text2,
        sidebar = p.sidebar,
        border = p.border,
        raised = p.raised,
        accent = p.accent,
        on_accent = p.on_accent,
        accent_text = p.accent_text,
        danger = p.danger,
    );
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == text) {
        return Ok(());
    }
    std::fs::create_dir_all(folder)?;
    std::fs::write(&path, text)
}

/// `text` with `keys` set in its `[Settings]` section, which is added when there isn't one.
fn set_ini(text: &str, keys: &[(&str, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let start = match lines.iter().position(|line| line.trim() == "[Settings]") {
        Some(start) => start,
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[Settings]".into());
            lines.len() - 1
        }
    };
    let end = lines.iter().skip(start + 1).position(|line| line.trim_start().starts_with('[')).map_or(lines.len(), |offset| start + 1 + offset);
    let mut insert_at = end;
    while insert_at > start + 1 && lines[insert_at - 1].trim().is_empty() {
        insert_at -= 1;
    }
    for (key, value) in keys {
        let line = format!("{key}={value}");
        let existing = lines[start + 1..insert_at].iter().position(|line| line.split_once('=').is_some_and(|(name, _)| name.trim() == *key));
        match existing {
            Some(offset) => lines[start + 1 + offset] = line,
            None => {
                lines.insert(insert_at, line);
                insert_at += 1;
            }
        }
    }
    lines.join("\n") + "\n"
}

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub ground: String,
    pub surface: String,
    pub sidebar: String,
    pub border: String,
    pub text: String,
    pub text2: String,
    /// Quieter than text2: captions, placeholders, headings in sidebars.
    pub text3: String,
    /// Section headings.
    pub heading: String,
    /// Disabled things and separators drawn as text.
    pub faint: String,
    pub accent: String,
    /// Text and icons on the accent.
    pub on_accent: String,
    pub accent_soft: String,
    pub accent_text: String,
    pub wall: String,
    pub bar: String,
    /// Cards a shade off the surface.
    pub raised: String,
    pub danger: String,
    /// Things to look at soon: an app busy for minutes, power draw.
    pub warning: String,
    /// Good news whatever the accent: connected, healthy.
    pub good: String,
    /// The colour shadows are made from.
    pub shadow: String,
}

impl Palette {
    /// Linen, light or dark, with an accent from [`ACCENTS`] (sage when unknown).
    pub fn linen(dark: bool, accent: &str, contrast: bool) -> Self {
        let custom = custom_accent(accent);
        let (name, light_hex, dark_hex, on_dark): (&str, &str, &str, &str) = match &custom {
            Some((name, light, dark, on_dark)) => (name, light, dark, on_dark),
            None => ACCENTS.iter().find(|(name, ..)| *name == accent).copied().unwrap_or(ACCENTS[0]),
        };
        let mut palette = if dark {
            let surface = "#26241F";
            Self {
                ground: "#1A1916".into(),
                surface: surface.into(),
                sidebar: "#201E1A".into(),
                border: "#3A3731".into(),
                text: "#ECE8E1".into(),
                text2: "#A8A196".into(),
                text3: "#8A8378".into(),
                heading: "#D9D3C9".into(),
                faint: "#5C574F".into(),
                accent: dark_hex.into(),
                on_accent: on_dark.into(),
                accent_soft: mix(dark_hex, surface, 0.22),
                accent_text: mix(dark_hex, "#FFFFFF", 0.82),
                wall: "#1F1D19".into(),
                bar: "rgba(38, 36, 31, 0.78)".into(),
                raised: "#2E2B26".into(),
                danger: "#E5806B".into(),
                warning: "#E3B866".into(),
                good: "#7FB89A".into(),
                shadow: "#000000".into(),
            }
        } else {
            // Sage's soft and text shades are the design's own; the others are mixed the same way.
            let (soft, text) = if name == "sage" { ("#E1EDE6".to_owned(), "#336650".to_owned()) } else { (mix(light_hex, "#FFFFFF", 0.14), mix(light_hex, "#000000", 0.84)) };
            Self {
                ground: "#F6F4EF".into(),
                surface: "#FFFFFF".into(),
                sidebar: "#EFECE5".into(),
                border: "#E2DDD3".into(),
                text: "#24221E".into(),
                text2: "#68625A".into(),
                text3: "#7A736A".into(),
                heading: "#3E3A34".into(),
                faint: "#C9C2B6".into(),
                accent: light_hex.into(),
                on_accent: "#FFFFFF".into(),
                accent_soft: soft,
                accent_text: text,
                wall: "#E7E1D6".into(),
                bar: "rgba(255, 253, 249, 0.75)".into(),
                raised: "#FBFAF7".into(),
                danger: "#C4503C".into(),
                warning: "#A9741E".into(),
                good: "#336650".into(),
                shadow: "#463A28".into(),
            }
        };
        if contrast {
            let (text2, text3, border, faint) = if dark { ("#CFC8BD", "#B3AC9F", "#5A554D", "#7D776C") } else { ("#4A453F", "#57524B", "#C2BAAC", "#9E978B") };
            (palette.text2, palette.text3, palette.border, palette.faint) = (text2.into(), text3.into(), border.into(), faint.into());
        }
        palette
    }

    /// Sets the token called `name` to `value`; false when there is no such token.
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        let field = match name {
            "ground" => &mut self.ground,
            "surface" => &mut self.surface,
            "sidebar" => &mut self.sidebar,
            "border" => &mut self.border,
            "text" => &mut self.text,
            "text2" => &mut self.text2,
            "text3" => &mut self.text3,
            "heading" => &mut self.heading,
            "faint" => &mut self.faint,
            "accent" => &mut self.accent,
            "on_accent" => &mut self.on_accent,
            "accent_soft" => &mut self.accent_soft,
            "accent_text" => &mut self.accent_text,
            "wall" => &mut self.wall,
            "bar" => &mut self.bar,
            "raised" => &mut self.raised,
            "danger" => &mut self.danger,
            "warning" => &mut self.warning,
            "good" => &mut self.good,
            "shadow" => &mut self.shadow,
            _ => return false,
        };
        *field = value.to_owned();
        true
    }

    /// GTK CSS `@define-color` lines, so stylesheets say `@accent` instead of a hex value.
    pub fn gtk_css(&self) -> String {
        self.tokens().iter().map(|(name, value)| format!("@define-color {name} {value};\n")).collect()
    }

    pub fn tokens(&self) -> [(&'static str, &str); 20] {
        [
            ("ground", &self.ground),
            ("surface", &self.surface),
            ("sidebar", &self.sidebar),
            ("border", &self.border),
            ("text", &self.text),
            ("text2", &self.text2),
            ("text3", &self.text3),
            ("heading", &self.heading),
            ("faint", &self.faint),
            ("accent", &self.accent),
            ("on_accent", &self.on_accent),
            ("accent_soft", &self.accent_soft),
            ("accent_text", &self.accent_text),
            ("wall", &self.wall),
            ("bar", &self.bar),
            ("raised", &self.raised),
            ("danger", &self.danger),
            ("warning", &self.warning),
            ("good", &self.good),
            ("shadow", &self.shadow),
        ]
    }
}

/// An accent given as `#RRGGBB` instead of a name: it is the light theme's accent as it is, and the dark theme's is
/// the same hue lightened, with the text on it made nearly black. `None` for a name.
fn custom_accent(accent: &str) -> Option<(String, String, String, String)> {
    crate::looks::is_hex(accent).then(|| ("custom".to_owned(), accent.to_ascii_uppercase(), mix(accent, "#FFFFFF", 0.65), mix(accent, "#000000", 0.15)))
}

fn channels(hex: &str) -> [f64; 3] {
    let hex = hex.trim_start_matches('#');
    let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2).unwrap_or("00"), 16).map_or(0.0, f64::from);
    [channel(0), channel(2), channel(4)]
}

/// A `#RRGGBB` colour's channels, from 0 to 1, for cairo.
pub fn rgb(hex: &str) -> [f64; 3] {
    channels(hex).map(|channel| channel / 255.0)
}

/// `amount` of `a` and the rest of `b`, as `#RRGGBB`.
pub fn mix(a: &str, b: &str, amount: f64) -> String {
    let (a, b) = (channels(a), channels(b));
    let mixed: Vec<String> = (0..3).map(|index| format!("{:02X}", (a[index] * amount + b[index] * (1.0 - amount)).round() as u8)).collect();
    format!("#{}", mixed.concat())
}

/// A stylesheet adjusted for the theme: the chosen font where the Linen stack is named, font sizes scaled
/// for the text size, and the faint `alpha(@text, …)` lines and fills made stronger for more contrast.
pub fn adjust(css: &str, theme: &Theme) -> String {
    let css = css.replace("\"Geist\", \"Inter\", sans-serif", &theme.font_stack());
    let scale = theme.text_scale();
    let mut out = String::with_capacity(css.len());
    let mut rest = css.as_str();
    while let Some(at) = rest.find("font-size:") {
        let (before, after) = rest.split_at(at + "font-size:".len());
        out.push_str(before);
        let spaces = after.len() - after.trim_start().len();
        let digits = after[spaces..].find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(0);
        let number = &after[spaces..spaces + digits];
        match (number.parse::<f64>(), after[spaces + digits..].starts_with("px")) {
            (Ok(size), true) if (scale - 1.0).abs() > 0.001 => {
                out.push_str(&after[..spaces]);
                out.push_str(&format!("{}", (size * scale * 10.0).round() / 10.0));
            }
            _ => out.push_str(&after[..spaces + digits]),
        }
        rest = &after[spaces + digits..];
    }
    out.push_str(rest);
    if !theme.contrast {
        return out;
    }
    let mut boosted = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(at) = rest.find("alpha(@text, ") {
        let (before, after) = rest.split_at(at + "alpha(@text, ".len());
        boosted.push_str(before);
        let end = after.find(')').unwrap_or(0);
        match after[..end].trim().parse::<f64>() {
            Ok(alpha) => boosted.push_str(&format!("{:.2}", (alpha * 1.8).min(1.0))),
            Err(_) => boosted.push_str(&after[..end]),
        }
        rest = &after[end..];
    }
    boosted.push_str(rest);
    boosted
}

#[cfg(feature = "style")]
pub use style::{Look, install, look, on_change, redraw_on_change, set_source};

#[cfg(feature = "style")]
mod style {
    //! The live part: every stylesheet a tool installs is rebuilt when `theme.toml` or the night flag changes.

    use std::cell::RefCell;
    use std::rc::Rc;

    use gtk::prelude::*;

    use super::{Palette, Theme, adjust, night_path};

    /// The theme as it is now, with its palette worked out.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Look {
        pub theme: Theme,
        pub dark: bool,
        pub palette: Palette,
    }

    impl Look {
        fn now() -> Self {
            let theme = Theme::load();
            let dark = theme.dark();
            let palette = theme.palette_for(dark);
            Self { theme, dark, palette }
        }
    }

    type Listener = Rc<dyn Fn(&Look)>;

    #[derive(Default)]
    struct State {
        look: Option<Rc<Look>>,
        sheets: Vec<(gtk::CssProvider, &'static str)>,
        listeners: Vec<Listener>,
        watch: Option<crate::watch::Watch>,
    }

    thread_local!(static STATE: RefCell<State> = RefCell::default());

    /// The current look, for colours drawn in code.
    pub fn look() -> Rc<Look> {
        STATE.with_borrow_mut(|state| state.look.get_or_insert_with(|| Rc::new(Look::now())).clone())
    }

    /// Calls `changed` whenever the look changes (after the stylesheets are reloaded).
    pub fn on_change(changed: impl Fn(&Look) + 'static) {
        STATE.with_borrow_mut(|state| state.listeners.push(Rc::new(changed)));
    }

    /// Redraws `widget` when the look changes, for colours drawn in code.
    pub fn redraw_on_change(widget: &impl IsA<gtk::Widget>) {
        let widget = widget.downgrade();
        on_change(move |_| {
            if let Some(widget) = widget.upgrade() {
                widget.queue_draw();
            }
        });
    }

    /// Paints with a `#RRGGBB` token at `alpha`.
    pub fn set_source(cr: &gtk::cairo::Context, hex: &str, alpha: f64) {
        let [r, g, b] = super::rgb(hex);
        cr.set_source_rgba(r, g, b, alpha);
    }

    /// Loads `css` for the whole display with the theme's tokens, and keeps it current.
    pub fn install(css: &'static str) {
        let provider = gtk::CssProvider::new();
        let look = look();
        load(&provider, css, &look);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        prefer_dark(look.dark);
        STATE.with_borrow_mut(|state| {
            state.sheets.push((provider, css));
            if state.watch.is_none() {
                let mut paths = crate::config::paths("theme");
                paths.push(night_path());
                state.watch = Some(crate::watch::files(&paths, reload));
            }
        });
    }

    fn load(provider: &gtk::CssProvider, css: &str, look: &Look) {
        let mut text = look.palette.gtk_css();
        if look.theme.reduce_motion {
            text.push_str("* { transition-duration: 0s; animation: none; }\n");
        }
        text.push_str(crate::chrome::CSS);
        text.push_str(&adjust(css, &look.theme));
        provider.load_from_string(&text);
    }

    fn prefer_dark(dark: bool) {
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(dark);
        }
    }

    fn reload() {
        let look = Rc::new(Look::now());
        let (sheets, listeners) = STATE.with_borrow_mut(|state| {
            if state.look.as_deref() == Some(&look) {
                return (Vec::new(), Vec::new());
            }
            state.look = Some(look.clone());
            (state.sheets.clone(), state.listeners.clone())
        });
        if sheets.is_empty() && listeners.is_empty() {
            return;
        }
        prefer_dark(look.dark);
        for (provider, css) in &sheets {
            load(provider, css, &look);
        }
        for listener in listeners {
            listener(&look);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylesheets_follow_the_theme() {
        let theme = Theme { text_size: 115, font: "Inter".into(), contrast: true, ..Theme::default() };
        let css = "window { font-family: \"Geist\", \"Inter\", sans-serif; font-size: 13px; border: 1px solid alpha(@text, 0.06); } .big { font-size:22px; }";
        assert_eq!(adjust(css, &theme), "window { font-family: \"Inter\", \"Geist\", sans-serif; font-size: 15px; border: 1px solid alpha(@text, 0.11); } .big { font-size:25.3px; }");
        assert_eq!(adjust(css, &Theme::default()), css);
    }

    #[test]
    fn gtk_settings_keep_other_lines() {
        let keys = [("gtk-application-prefer-dark-theme", "1".to_owned()), ("gtk-font-name", "Inter 13".to_owned())];
        assert_eq!(set_ini("", &keys), "[Settings]\ngtk-application-prefer-dark-theme=1\ngtk-font-name=Inter 13\n");
        let text = "# mine\n[Settings]\ngtk-icon-theme-name=Adwaita\ngtk-font-name = Cantarell 11\n\n[Other]\na=b\n";
        assert_eq!(set_ini(text, &keys), "# mine\n[Settings]\ngtk-icon-theme-name=Adwaita\ngtk-font-name=Inter 13\ngtk-application-prefer-dark-theme=1\n\n[Other]\na=b\n");
    }

    #[test]
    fn accents_mix_their_shades() {
        assert_eq!(mix("#000000", "#FFFFFF", 0.5), "#808080");
        let light = Palette::linen(false, "blue", false);
        assert_eq!((light.accent.as_str(), light.on_accent.as_str()), ("#3D63E8", "#FFFFFF"));
        let dark = Palette::linen(true, "nonsense", false);
        assert_eq!(dark.accent, "#7FC1A1");
    }
}
