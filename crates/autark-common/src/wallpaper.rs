//! The wallpapers, drawn rather than shipped as pictures: Linen is `design/canvas/Wallpaper.dc.html` in the Autark
//! repo, and Ridge, Dusk and Daylight are the same hills in the colours of Settings' wallpaper cards. A scene is a TOML
//! file, so adding one is adding a file: the ones Autark ships are in `data/wallpapers/` (embedded here by `build.rs`),
//! yours are in `~/.config/autark/wallpapers/`, and a scene in either is chosen in Settings → Appearance. `theme.toml`
//! names one of them, or a picture's path.
//!
//! ```toml
//! name = "Dusk"
//! author = "…"            licence = "MIT OR Apache-2.0"       description = "Navy hills at sunset."
//! sky = "#1B2436"         # the sky, the band below the horizon, the sun or moon,
//! band = "#1F2A3E"
//! sun = "#E8914A"
//! hills = ["#36435E", "#2C3850", "#252F45", "#1E2639"]    # and the four hills, back to front
//!
//! [dark]                  # optional: the same four fields, used instead when the theme is dark
//! ```
//!
//! [`validate`] is what CI runs on every shipped scene.

use std::path::PathBuf;

use serde::Deserialize;

include!(concat!(env!("OUT_DIR"), "/builtin_wallpapers.rs"));

/// A scene's colours: the sky, the band below the horizon, the sun or moon, and the hills from back to front.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shades {
    pub sky: String,
    pub band: String,
    pub sun: String,
    pub hills: Vec<String>,
}

/// The file as written. Unknown keys are an error, so a misspelt colour is seen, not ignored.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct File {
    name: String,
    author: String,
    licence: String,
    description: String,
    sky: String,
    band: String,
    sun: String,
    hills: Vec<String>,
    dark: Option<Shades>,
}

/// A scene: its colours, and the ones it has for a dark theme if it says so.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scene {
    pub name: String,
    pub author: String,
    pub licence: String,
    pub description: String,
    pub light: Shades,
    pub dark: Option<Shades>,
}

impl Scene {
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: File = toml::from_str(text).map_err(|error| error.to_string())?;
        Ok(Scene {
            name: file.name,
            author: file.author,
            licence: file.licence,
            description: file.description,
            light: Shades { sky: file.sky, band: file.band, sun: file.sun, hills: file.hills },
            dark: file.dark,
        })
    }

    /// The colours to paint with: the dark ones when the theme is dark and the scene has them, else the scene's own.
    pub fn shades(&self, dark: bool) -> &Shades {
        match (&self.dark, dark) {
            (Some(shades), true) => shades,
            _ => &self.light,
        }
    }
}

/// The scene the design is drawn at.
pub const WIDTH: f64 = 1440.0;
pub const HEIGHT: f64 = 900.0;

/// How many hills a scene has colours for.
const HILLS: usize = 4;

/// Where the band starts, and the sun.
#[cfg(feature = "style")]
const HORIZON: f64 = 320.0;
#[cfg(feature = "style")]
const SUN: (f64, f64, f64) = (1040.0, 400.0, 88.0);

/// Each hill's ridge line, left to right; it's closed along the bottom.
#[cfg(feature = "style")]
const RIDGES: [&[(f64, f64)]; HILLS] = [
    &[(0.0, 560.0), (180.0, 470.0), (330.0, 520.0), (520.0, 400.0), (690.0, 500.0), (860.0, 430.0), (1010.0, 490.0), (1180.0, 380.0), (1330.0, 460.0), (1440.0, 420.0)],
    &[(0.0, 640.0), (140.0, 580.0), (300.0, 620.0), (470.0, 540.0), (640.0, 610.0), (800.0, 560.0), (980.0, 630.0), (1150.0, 560.0), (1300.0, 600.0), (1440.0, 570.0)],
    &[(0.0, 730.0), (200.0, 660.0), (380.0, 700.0), (560.0, 640.0), (760.0, 720.0), (940.0, 670.0), (1120.0, 730.0), (1300.0, 680.0), (1440.0, 710.0)],
    &[(0.0, 820.0), (240.0, 760.0), (480.0, 800.0), (720.0, 750.0), (960.0, 810.0), (1200.0, 770.0), (1440.0, 800.0)],
];

/// Your scenes, then the system's: the folders a scene can be in besides the ones built in.
pub fn dirs() -> Vec<PathBuf> {
    crate::catalog::dirs("wallpapers")
}

/// Every scene, with Linen first and the rest by id. A scene in an earlier place replaces one with the same id in a later
/// one (yours over the system's over the built-in ones), and a file that doesn't parse or has a colour that isn't one is
/// left out, so whatever is listed can be drawn.
pub fn list() -> Vec<(String, Scene)> {
    list_in(&dirs())
}

pub fn list_in(dirs: &[PathBuf]) -> Vec<(String, Scene)> {
    let mut all: Vec<(String, Scene)> = crate::catalog::collect(BUILTIN, dirs, |text| {
        let scene = Scene::parse(text)?;
        match validate(&scene, false).first() {
            Some(problem) => Err(problem.clone()),
            None => Ok(scene),
        }
    }).into_iter().collect();
    all.sort_by_key(|(id, _)| (id != "linen", id.clone()));
    all
}

/// The scene `name` names, if it names one rather than a picture.
pub fn find(name: &str) -> Option<Scene> {
    find_in(name, &dirs())
}

pub fn find_in(name: &str, dirs: &[PathBuf]) -> Option<Scene> {
    list_in(dirs).into_iter().find(|(id, _)| id == name).map(|(_, scene)| scene)
}

/// Everything wrong with `scene`, in words; empty when it is fine. `shipped` adds what a scene in this repository needs: a
/// licence and a description.
pub fn validate(scene: &Scene, shipped: bool) -> Vec<String> {
    use crate::looks::is_hex;

    let mut problems = Vec::new();
    if scene.name.trim().is_empty() {
        problems.push("it needs a name".to_owned());
    }
    if shipped {
        if scene.licence.trim().is_empty() {
            problems.push("it needs a licence (an SPDX id such as MIT OR Apache-2.0)".to_owned());
        }
        if scene.description.trim().is_empty() {
            problems.push("it needs a description".to_owned());
        }
    }
    for (table, shades) in [("", Some(&scene.light)), ("[dark] ", scene.dark.as_ref())] {
        let Some(shades) = shades else { continue };
        for (field, value) in [("sky", &shades.sky), ("band", &shades.band), ("sun", &shades.sun)] {
            if !is_hex(value) {
                problems.push(format!("{table}{field} = {value:?} is not #RRGGBB"));
            }
        }
        if shades.hills.len() != HILLS {
            problems.push(format!("{table}hills needs {} colours, back to front; it has {}", HILLS, shades.hills.len()));
        }
        for hill in shades.hills.iter().filter(|hill| !is_hex(hill)) {
            problems.push(format!("{table}hills has {hill:?}, which is not #RRGGBB"));
        }
    }
    problems
}

/// Whether `name` is a picture's path rather than a scene.
pub fn is_picture(name: &str) -> bool {
    name.starts_with('/') || name.starts_with('~')
}

/// A picture's path with `~` worked out.
pub fn picture_path(name: &str) -> std::path::PathBuf {
    match name.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default().join(rest),
        None => name.into(),
    }
}

/// Paints `scene` to cover `width` by `height`, centred, as a picture set to cover would be.
#[cfg(feature = "style")]
pub fn draw(cr: &gtk::cairo::Context, scene: &Shades, width: f64, height: f64) {
    use crate::theme::set_source;

    let scale = (width / WIDTH).max(height / HEIGHT);
    cr.save().ok();
    cr.translate((width - WIDTH * scale) / 2.0, (height - HEIGHT * scale) / 2.0);
    cr.scale(scale, scale);
    set_source(cr, &scene.sky, 1.0);
    cr.rectangle(0.0, 0.0, WIDTH, HEIGHT);
    cr.fill().ok();
    set_source(cr, &scene.band, 1.0);
    cr.rectangle(0.0, HORIZON, WIDTH, HEIGHT - HORIZON);
    cr.fill().ok();
    set_source(cr, &scene.sun, 1.0);
    cr.arc(SUN.0, SUN.1, SUN.2, 0.0, std::f64::consts::TAU);
    cr.fill().ok();
    for (ridge, colour) in RIDGES.iter().zip(&scene.hills) {
        for (index, &(x, y)) in ridge.iter().enumerate() {
            if index == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        cr.line_to(WIDTH, HEIGHT);
        cr.line_to(0.0, HEIGHT);
        cr.close_path();
        set_source(cr, colour, 1.0);
        cr.fill().ok();
    }
    cr.restore().ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(id: &str) -> Scene {
        let (_, text) = BUILTIN.iter().find(|(name, _)| *name == id).unwrap_or_else(|| panic!("no built-in scene {id}"));
        Scene::parse(text).unwrap()
    }

    #[test]
    fn every_scene_in_the_repository_is_valid() {
        assert!(BUILTIN.len() >= 4, "the shipped scenes should have been embedded");
        for (id, text) in BUILTIN {
            let scene = Scene::parse(text).unwrap_or_else(|error| panic!("{id}.toml: {error}"));
            let problems = validate(&scene, true);
            assert!(problems.is_empty(), "{id}.toml: {problems:#?}");
        }
    }

    #[test]
    fn the_four_scenes_are_unchanged() {
        let linen = builtin("linen");
        assert_eq!((linen.light.sky.as_str(), linen.light.sun.as_str(), linen.light.hills[3].as_str()), ("#EFE9DF", "#A9C7B6", "#9DB5A5"));
        // Linen has its own dark colours; the others stay as picked.
        assert_eq!(linen.shades(true).sky, "#1E1C18");
        assert_eq!(linen.shades(true).hills, ["#2B2823", "#322E28", "#2E4237", "#26362E"]);
        assert_eq!(builtin("dusk").shades(true), builtin("dusk").shades(false));
        assert_eq!(builtin("ridge").light.sun, "#4C8DF7");
        assert_eq!(builtin("daylight").light.band, "#D3DFEC");
        assert_eq!(list_in(&[]).iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["linen", "daylight", "dusk", "ridge"]);
    }

    #[test]
    fn names_are_scenes_or_pictures() {
        assert_eq!(find_in("dusk", &[]).map(|scene| scene.name), Some("Dusk".to_owned()));
        assert!(find_in("/home/user/sea.jpg", &[]).is_none());
        assert!(is_picture("~/Pictures/sea.jpg") && !is_picture("linen"));
        assert!(picture_path("~/sea.jpg").ends_with("sea.jpg"));
    }

    #[test]
    fn mistakes_in_the_file_are_named() {
        assert!(Scene::parse("sky = \"#000000\"\nsunn = \"#fff\"").is_err(), "an unknown key is an error");
        let bad = Scene::parse("name = \"Bad\"\nsky = \"blue\"\nband = \"#12345\"\nsun = \"#FFFFFF\"\nhills = [\"#000000\", \"red\"]\n[dark]\nsky = \"#000000\"\n").unwrap();
        let problems = validate(&bad, false);
        assert!(problems.iter().any(|problem| problem.contains("sky = \"blue\"")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("band")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("hills needs 4") && !problem.contains("[dark]")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("\"red\"")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.starts_with("[dark] band")), "{problems:?}");
        assert!(validate(&Scene::default(), false).iter().any(|problem| problem.contains("name")));
        let bare = builtin("ridge");
        let bare = Scene { licence: String::new(), description: String::new(), ..bare };
        assert!(validate(&bare, false).is_empty());
        assert_eq!(validate(&bare, true).len(), 2);
    }

    #[test]
    fn your_scenes_replace_the_shipped_ones_with_the_same_id() {
        let dir = std::env::temp_dir().join(format!("autark-wallpapers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mine = |name: &str| format!("name = \"{name}\"\nsky = \"#000000\"\nband = \"#111111\"\nsun = \"#FFFFFF\"\nhills = [\"#222222\", \"#333333\", \"#444444\", \"#555555\"]\n");
        std::fs::write(dir.join("dusk.toml"), mine("My Dusk")).unwrap();
        std::fs::write(dir.join("mine.toml"), mine("Mine")).unwrap();
        std::fs::write(dir.join("colourless.toml"), "name = \"No colours\"\n").unwrap();
        std::fs::write(dir.join("broken.toml"), "this is not toml = = =\n").unwrap();
        let ids: Vec<String> = list_in(std::slice::from_ref(&dir)).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids[0], "linen");
        assert!(ids.contains(&"mine".to_owned()) && !ids.contains(&"broken".to_owned()) && !ids.contains(&"colourless".to_owned()), "{ids:?}");
        assert_eq!(find_in("dusk", std::slice::from_ref(&dir)).unwrap().name, "My Dusk");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(find_in("dusk", &[]).unwrap().name, "Dusk");
    }
}
