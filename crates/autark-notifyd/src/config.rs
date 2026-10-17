//! `notifyd.toml`: every field is optional.

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifyd {
    /// How long a banner stays when the app doesn't say.
    pub banner_seconds: u32,
    /// Banners shown at once; older ones go straight to the centre.
    pub max_banners: usize,
}

impl Default for Notifyd {
    fn default() -> Self {
        Self { banner_seconds: 5, max_banners: 3 }
    }
}
