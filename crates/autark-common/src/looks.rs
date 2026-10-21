//! Looks: a named bundle of colours, accent, wallpaper and font. A look is a TOML file, so adding one is adding a file:
//! the ones Autark ships are in `data/looks/` (embedded here by `build.rs`), yours are in `~/.config/autark/looks/`, and
//! a look in either is chosen in Settings → Appearance. Choosing one writes its fields into `theme.toml`; the palette it
//! changes (`[light]` and `[dark]` tables of token names) is applied on top of Linen whenever `theme.toml` says
//! `look = "<id>"`.
//!
//! ```toml
//! name = "Graphite"
//! author = "…"            licence = "MIT OR Apache-2.0"       description = "Cool greys."
//! mode = "light"          # light, dark or auto; leave it out to leave the mode as it is
//! accent = "blue"         # a name from `ACCENTS`, or "#RRGGBB"
//! wallpaper = "daylight"  # a scene, or the path to a picture
//! font = "Inter"
//!
//! [light]                 # any of the palette's tokens, as "#RRGGBB" (the bar's is "rgba(r, g, b, a)")
//! surface = "#FFFFFF"
//! text = "#1D2026"
//! ```
//!
//! [`validate`] is what CI runs on every shipped look: the file's shape, every colour's syntax, and that the text is
//! readable (WCAG AA, 4.5:1) on the surfaces it is drawn on, in each mode the look defines.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::theme::{Mode, Palette, Theme, rgb};

include!(concat!(env!("OUT_DIR"), "/builtin_looks.rs"));

/// The parsed file. Unknown keys are an error, so a misspelt token is seen, not ignored.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    pub author: String,
    pub licence: String,
    pub description: String,
    pub mode: Option<Mode>,
    pub accent: Option<String>,
    pub wallpaper: Option<String>,
    pub font: Option<String>,
    pub light: BTreeMap<String, String>,
    pub dark: BTreeMap<String, String>,
}

impl Preset {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| error.to_string())
    }

    /// The palette this look gives in the light or dark theme: Linen with the look's accent, then its own tokens.
    /// `contrast` is the "Increase contrast" switch, applied before the look's tokens so the look wins.
    pub fn palette(&self, dark: bool, contrast: bool) -> Palette {
        let mut palette = Palette::linen(dark, self.accent.as_deref().unwrap_or("sage"), contrast);
        self.apply(&mut palette, dark);
        palette
    }

    /// Writes this look's tokens for the mode over `palette`. Names it doesn't know are skipped (validation reports them).
    pub fn apply(&self, palette: &mut Palette, dark: bool) {
        for (token, value) in if dark { &self.dark } else { &self.light } {
            palette.set(token, value);
        }
    }

    /// The themes this look is meant to be seen in: one, or both when it leaves the mode open.
    fn modes(&self) -> Vec<bool> {
        match self.mode {
            Some(Mode::Light) => vec![false],
            Some(Mode::Dark) => vec![true],
            Some(Mode::Auto) | None => vec![false, true],
        }
    }

    /// Whether `theme` is this look as chosen, with nothing changed since: the same id and the same fields. A theme that
    /// names no look is Linen, which is what Autark is until one is chosen.
    pub fn is_current(&self, id: &str, theme: &Theme) -> bool {
        (theme.look == id || (theme.look.is_empty() && id == "linen"))
            && self.mode.is_none_or(|mode| mode == theme.theme)
            && self.accent.as_deref().is_none_or(|accent| accent == theme.accent)
            && self.wallpaper.as_deref().is_none_or(|wallpaper| wallpaper == theme.wallpaper)
            && self.font.as_deref().is_none_or(|font| font == theme.font)
    }
}

/// Your looks, then the system's: the folders a look can be in besides the ones built in.
pub fn dirs() -> Vec<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    config.map(|dir| dir.join("autark/looks")).into_iter().chain([PathBuf::from("/usr/share/autark/looks")]).collect()
}

/// Every look, with Linen first and the rest by id. A look in an earlier place replaces one with the same id in a later one
/// (yours over the system's over the built-in ones), and a file that doesn't parse is left out.
pub fn list() -> Vec<(String, Preset)> {
    list_in(&dirs())
}

pub fn list_in(dirs: &[PathBuf]) -> Vec<(String, Preset)> {
    let mut found: BTreeMap<String, Preset> = BTreeMap::new();
    for (id, text) in BUILTIN {
        if let Ok(preset) = Preset::parse(text) {
            found.insert((*id).to_owned(), preset);
        }
    }
    // The system's folder then yours, so the later one wins; `dirs` is yours first.
    for dir in dirs.iter().rev() {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for path in entries.flatten().map(|entry| entry.path()) {
            let (Some(id), true) = (path.file_stem().and_then(|stem| stem.to_str()), path.extension().is_some_and(|ext| ext == "toml")) else { continue };
            match std::fs::read_to_string(&path).map_err(|error| error.to_string()).and_then(|text| Preset::parse(&text)) {
                Ok(preset) => {
                    found.insert(id.to_owned(), preset);
                }
                Err(error) => eprintln!("{}: {error}", path.display()),
            }
        }
    }
    let mut all: Vec<(String, Preset)> = found.into_iter().collect();
    all.sort_by_key(|(id, _)| (id != "linen", id.clone()));
    all
}

pub fn find(id: &str) -> Option<Preset> {
    find_in(id, &dirs())
}

pub fn find_in(id: &str, dirs: &[PathBuf]) -> Option<Preset> {
    list_in(dirs).into_iter().find(|(found, _)| found == id).map(|(_, preset)| preset)
}

/// A name that can be a file name: lower-case letters, digits and dashes, from what a person typed.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// The WCAG contrast ratio between two `#RRGGBB` colours: 1 for the same colour, 21 for black on white.
pub fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn luminance(hex: &str) -> f64 {
    let [r, g, b] = rgb(hex).map(|channel| if channel <= 0.03928 { channel / 12.92 } else { ((channel + 0.055) / 1.055).powf(2.4) });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// `#RRGGBB`, with exactly six hex digits.
pub fn is_hex(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// A palette token's value: `#RRGGBB`, or `rgba(r, g, b, a)` for the translucent ones.
fn is_color(text: &str) -> bool {
    if is_hex(text) {
        return true;
    }
    let Some(inner) = text.strip_prefix("rgba(").and_then(|rest| rest.strip_suffix(')')) else { return false };
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    parts.len() == 4 && parts[..3].iter().all(|part| part.parse::<u8>().is_ok()) && parts[3].parse::<f64>().is_ok_and(|alpha| (0.0..=1.0).contains(&alpha))
}

/// The pairs that must be readable: what is drawn on what, and the least ratio each needs.
const READABLE: [(&str, &str, f64); 3] = [("text", "surface", 4.5), ("text2", "surface", 4.5), ("on_accent", "accent", 4.5)];

/// Everything wrong with `preset`, in words; empty when it is fine. `shipped` adds what a look in this repository needs: a
/// name, a licence and a description, and a wallpaper that is one of the scenes.
pub fn validate(preset: &Preset, shipped: bool) -> Vec<String> {
    let mut problems = Vec::new();
    if preset.name.trim().is_empty() {
        problems.push("it needs a name".to_owned());
    }
    if shipped {
        if preset.licence.trim().is_empty() {
            problems.push("it needs a licence (an SPDX id such as MIT OR Apache-2.0)".to_owned());
        }
        if preset.description.trim().is_empty() {
            problems.push("it needs a description".to_owned());
        }
        if let Some(wallpaper) = &preset.wallpaper
            && crate::wallpaper::scene(wallpaper).is_none()
        {
            problems.push(format!("wallpaper {wallpaper:?} is not one of the scenes"));
        }
    }
    if let Some(accent) = &preset.accent
        && !is_hex(accent)
        && !crate::theme::ACCENTS.iter().any(|(name, ..)| name == accent)
    {
        problems.push(format!("accent {accent:?} is neither a name from the accent list nor #RRGGBB"));
    }
    let known: Vec<&str> = Palette::linen(false, "sage", false).tokens().iter().map(|(name, _)| *name).collect();
    for (table, tokens) in [("light", &preset.light), ("dark", &preset.dark)] {
        for (token, value) in tokens {
            if !known.contains(&token.as_str()) {
                problems.push(format!("[{table}] has no token called {token:?}"));
            } else if !is_color(value) {
                problems.push(format!("[{table}] {token} = {value:?} is not #RRGGBB or rgba(r, g, b, a)"));
            }
        }
    }
    if !problems.is_empty() {
        return problems;
    }
    for dark in preset.modes() {
        let palette = preset.palette(dark, false);
        let tokens = palette.tokens();
        let value = |name: &str| tokens.iter().find(|(token, _)| *token == name).map(|(_, value)| *value).unwrap_or("#000000");
        for (text, ground, least) in READABLE {
            let ratio = contrast(value(text), value(ground));
            if ratio < least {
                problems.push(format!("{} theme: {text} {} on {ground} {} is {ratio:.2}:1, below {least}:1", if dark { "dark" } else { "light" }, value(text), value(ground)));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(id: &str) -> Preset {
        let (_, text) = BUILTIN.iter().find(|(name, _)| *name == id).unwrap_or_else(|| panic!("no built-in look {id}"));
        Preset::parse(text).unwrap()
    }

    #[test]
    fn every_look_in_the_repository_is_valid_and_readable() {
        assert!(BUILTIN.len() >= 4, "the shipped looks should have been embedded");
        for (id, text) in BUILTIN {
            let preset = Preset::parse(text).unwrap_or_else(|error| panic!("{id}.toml: {error}"));
            let problems = validate(&preset, true);
            assert!(problems.is_empty(), "{id}.toml: {problems:#?}");
        }
    }

    #[test]
    fn linen_is_the_default_and_changes_nothing() {
        let linen = builtin("linen");
        assert!(linen.light.is_empty() && linen.dark.is_empty());
        let theme = Theme::default();
        assert_eq!((linen.mode, linen.accent.as_deref(), linen.wallpaper.as_deref(), linen.font.as_deref()), (Some(Mode::Light), Some(theme.accent.as_str()), Some(theme.wallpaper.as_str()), Some(theme.font.as_str())));
        assert_eq!(linen.palette(false, false), Palette::linen(false, "sage", false));
        assert_eq!(linen.palette(true, false), Palette::linen(true, "sage", false));
    }

    #[test]
    fn a_look_changes_only_the_tokens_it_names() {
        let graphite = builtin("graphite");
        let (base, look) = (Palette::linen(false, "blue", false), graphite.palette(false, false));
        assert_eq!(look.text, "#1D2026");
        assert_eq!(look.surface, "#FFFFFF");
        assert_eq!((&look.accent, &look.danger, &look.good), (&base.accent, &base.danger, &base.good));
        // Its dark table is empty, so in the dark theme it is plain Linen with its accent.
        assert_eq!(graphite.palette(true, false), Palette::linen(true, "blue", false));
    }

    #[test]
    fn contrast_is_the_wcag_ratio() {
        assert!((contrast("#000000", "#FFFFFF") - 21.0).abs() < 1e-9);
        assert!((contrast("#336650", "#336650") - 1.0).abs() < 1e-9);
        assert!(contrast("#767676", "#FFFFFF") > 4.5 && contrast("#777777", "#FFFFFF") < 4.5, "#767676 is the lightest grey that passes on white");
    }

    #[test]
    fn a_look_that_is_hard_to_read_is_refused() {
        let pale = Preset::parse("name = \"Pale\"\nmode = \"light\"\n[light]\ntext = \"#CCCCCC\"\n").unwrap();
        let problems = validate(&pale, false);
        assert!(problems.iter().any(|problem| problem.contains("text") && problem.contains("below 4.5:1")), "{problems:?}");
        // Dark text on a dark accent button is just as unreadable.
        let muddy = Preset::parse("name = \"Muddy\"\nmode = \"light\"\n[light]\naccent = \"#444444\"\non_accent = \"#555555\"\n").unwrap();
        assert!(validate(&muddy, false).iter().any(|problem| problem.contains("on_accent")));
    }

    #[test]
    fn mistakes_in_the_file_are_named() {
        assert!(Preset::parse("nmae = \"x\"").is_err(), "an unknown key is an error");
        let bad = Preset::parse("name = \"Bad\"\naccent = \"red\"\n[light]\ntxt = \"#000000\"\nsurface = \"white\"\nbar = \"rgba(1, 2, 3)\"\n").unwrap();
        let problems = validate(&bad, false);
        assert!(problems.iter().any(|problem| problem.contains("accent \"red\"")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("no token called \"txt\"")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("surface") && problem.contains("not #RRGGBB")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("bar")), "{problems:?}");
        assert!(validate(&Preset::default(), false).iter().any(|problem| problem.contains("name")));
    }

    #[test]
    fn a_look_for_this_repository_needs_a_licence_and_a_real_scene() {
        let bare = Preset::parse("name = \"Bare\"\nwallpaper = \"nowhere\"\n").unwrap();
        assert!(validate(&bare, false).is_empty());
        let problems = validate(&bare, true);
        assert!(problems.iter().any(|problem| problem.contains("licence")) && problems.iter().any(|problem| problem.contains("description")) && problems.iter().any(|problem| problem.contains("nowhere")), "{problems:?}");
    }

    #[test]
    fn a_hex_accent_makes_its_own_dark_and_light_shades() {
        let light = Palette::linen(false, "#B5451B", false);
        assert_eq!(light.accent, "#B5451B");
        let dark = Palette::linen(true, "#B5451B", false);
        assert_ne!(dark.accent, "#B5451B");
        assert!(contrast(&dark.on_accent, &dark.accent) > 4.5, "{} on {}", dark.on_accent, dark.accent);
    }

    #[test]
    fn your_looks_replace_the_shipped_ones_with_the_same_id() {
        let dir = std::env::temp_dir().join(format!("autark-looks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("graphite.toml"), "name = \"My Graphite\"\n").unwrap();
        std::fs::write(dir.join("mine.toml"), "name = \"Mine\"\n").unwrap();
        std::fs::write(dir.join("broken.toml"), "this is not toml = = =\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let all = list_in(std::slice::from_ref(&dir));
        let ids: Vec<&str> = all.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids[0], "linen", "Linen first");
        assert!(ids.contains(&"mine") && ids.contains(&"midnight") && !ids.contains(&"broken") && !ids.contains(&"notes"), "{ids:?}");
        assert_eq!(find_in("graphite", std::slice::from_ref(&dir)).unwrap().name, "My Graphite");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(find_in("graphite", &[]).unwrap().name, "Graphite");
    }

    #[test]
    fn a_name_becomes_a_file_name() {
        assert_eq!(slug("  My  Cool Look! "), "my-cool-look");
        assert_eq!(slug("日本"), "");
        assert_eq!(slug("a-b_c"), "a-b-c");
    }

    #[test]
    fn a_look_is_current_until_something_it_sets_is_changed() {
        let graphite = builtin("graphite");
        let mut theme = Theme { look: "graphite".into(), theme: Mode::Light, accent: "blue".into(), wallpaper: "daylight".into(), font: "Inter".into(), ..Theme::default() };
        assert!(graphite.is_current("graphite", &theme));
        theme.accent = "coral".into();
        assert!(!graphite.is_current("graphite", &theme));
        assert!(!graphite.is_current("midnight", &Theme { look: "graphite".into(), ..theme }));
    }

    #[test]
    fn a_fresh_install_is_linen() {
        assert!(builtin("linen").is_current("linen", &Theme::default()));
        assert!(!builtin("graphite").is_current("graphite", &Theme::default()));
        // Linen with the accent changed is no longer Linen.
        assert!(!builtin("linen").is_current("linen", &Theme { accent: "coral".into(), ..Theme::default() }));
    }
}
