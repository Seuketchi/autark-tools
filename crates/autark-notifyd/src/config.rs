//! `notifyd.toml`: every field is optional.

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifyd {
    /// How long a banner stays when the app doesn't say.
    pub banner_seconds: u32,
    /// Banners shown at once; older ones go straight to the centre.
    pub max_banners: usize,
    /// Kith's suggestions to quit runaway apps, from a reading of `autark-stats` once a minute.
    pub suggestions: bool,
    /// The note in the Pinned note widget, as a path inside the Notes vault. Without it, the
    /// first note pinned in Notes.
    pub note: Option<String>,
    /// Where to show the weather for. Weather stays off until this is set and switched on in
    /// Edit widgets, since each update sends the location to Open-Meteo.
    pub weather: Option<Location>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    /// Shown above the temperature.
    pub place: Option<String>,
}

impl Default for Notifyd {
    fn default() -> Self {
        Self { banner_seconds: 5, max_banners: 3, suggestions: true, note: None, weather: None }
    }
}
