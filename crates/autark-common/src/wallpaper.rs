//! The four wallpapers, drawn rather than shipped as pictures: Linen is `design/canvas/Wallpaper.dc.html` in
//! the Autark repo, and Ridge, Dusk and Daylight are the same hills in the colours of Settings' wallpaper
//! cards. `theme.toml` names one of them, or a picture's path.

/// A scene's colours: the sky, the band below the horizon, the sun or moon, and the hills from back to front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scene {
    pub id: &'static str,
    pub label: &'static str,
    pub sky: &'static str,
    pub band: &'static str,
    pub sun: &'static str,
    pub hills: [&'static str; 4],
}

pub const SCENES: [Scene; 4] = [
    Scene { id: "linen", label: "Linen", sky: "#EFE9DF", band: "#E9E2D6", sun: "#A9C7B6", hills: ["#DDD4C5", "#D0C5B3", "#B9CBBE", "#9DB5A5"] },
    Scene { id: "ridge", label: "Ridge", sky: "#16181E", band: "#191B22", sun: "#4C8DF7", hills: ["#2F3440", "#262A34", "#20232B", "#1A1C22"] },
    Scene { id: "dusk", label: "Dusk", sky: "#1B2436", band: "#1F2A3E", sun: "#E8914A", hills: ["#36435E", "#2C3850", "#252F45", "#1E2639"] },
    Scene { id: "daylight", label: "Daylight", sky: "#DCE6F2", band: "#D3DFEC", sun: "#FFFFFF", hills: ["#C3D2E3", "#AFC2D8", "#9EB4CD", "#8EA6C2"] },
];

/// Linen at night: the same hills in the dark theme's warm greys and a muted sage sun. Used when the wallpaper is Linen and the theme is dark;
/// the other scenes are the ones you picked and stay as they are.
pub const LINEN_DARK: Scene = Scene { id: "linen", label: "Linen", sky: "#1E1C18", band: "#221F1B", sun: "#4F7561", hills: ["#2B2823", "#322E28", "#2E4237", "#26362E"] };

/// The scene the design is drawn at.
pub const WIDTH: f64 = 1440.0;
pub const HEIGHT: f64 = 900.0;

/// Where the band starts, and the sun.
#[cfg(feature = "style")]
const HORIZON: f64 = 320.0;
#[cfg(feature = "style")]
const SUN: (f64, f64, f64) = (1040.0, 400.0, 88.0);

/// Each hill's ridge line, left to right; it's closed along the bottom.
#[cfg(feature = "style")]
const RIDGES: [&[(f64, f64)]; 4] = [
    &[(0.0, 560.0), (180.0, 470.0), (330.0, 520.0), (520.0, 400.0), (690.0, 500.0), (860.0, 430.0), (1010.0, 490.0), (1180.0, 380.0), (1330.0, 460.0), (1440.0, 420.0)],
    &[(0.0, 640.0), (140.0, 580.0), (300.0, 620.0), (470.0, 540.0), (640.0, 610.0), (800.0, 560.0), (980.0, 630.0), (1150.0, 560.0), (1300.0, 600.0), (1440.0, 570.0)],
    &[(0.0, 730.0), (200.0, 660.0), (380.0, 700.0), (560.0, 640.0), (760.0, 720.0), (940.0, 670.0), (1120.0, 730.0), (1300.0, 680.0), (1440.0, 710.0)],
    &[(0.0, 820.0), (240.0, 760.0), (480.0, 800.0), (720.0, 750.0), (960.0, 810.0), (1200.0, 770.0), (1440.0, 800.0)],
];

/// The scene `name` names, if it names one rather than a picture.
pub fn scene(name: &str) -> Option<&'static Scene> {
    SCENES.iter().find(|scene| scene.id == name)
}

/// [`scene`], with Linen drawn in its dark colours when `dark`.
pub fn scene_for(name: &str, dark: bool) -> Option<&'static Scene> {
    match scene(name) {
        Some(found) if found.id == "linen" && dark => Some(&LINEN_DARK),
        other => other,
    }
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
pub fn draw(cr: &gtk::cairo::Context, scene: &Scene, width: f64, height: f64) {
    use crate::theme::set_source;

    let scale = (width / WIDTH).max(height / HEIGHT);
    cr.save().ok();
    cr.translate((width - WIDTH * scale) / 2.0, (height - HEIGHT * scale) / 2.0);
    cr.scale(scale, scale);
    set_source(cr, scene.sky, 1.0);
    cr.rectangle(0.0, 0.0, WIDTH, HEIGHT);
    cr.fill().ok();
    set_source(cr, scene.band, 1.0);
    cr.rectangle(0.0, HORIZON, WIDTH, HEIGHT - HORIZON);
    cr.fill().ok();
    set_source(cr, scene.sun, 1.0);
    cr.arc(SUN.0, SUN.1, SUN.2, 0.0, std::f64::consts::TAU);
    cr.fill().ok();
    for (ridge, colour) in RIDGES.iter().zip(scene.hills) {
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

    #[test]
    fn names_are_scenes_or_pictures() {
        assert_eq!(scene("dusk").map(|scene| scene.label), Some("Dusk"));
        assert!(scene("/home/user/sea.jpg").is_none());
        assert!(is_picture("~/Pictures/sea.jpg") && !is_picture("linen"));
        assert!(picture_path("~/sea.jpg").ends_with("sea.jpg"));
    }
}
