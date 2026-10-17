//! `dock.toml`: every field is optional.

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dock {
    /// Icon size in pixels, from 32 to 80.
    pub size: i32,
    /// Icons grow to half again their size under the pointer.
    pub magnify: bool,
    /// Desktop file names, in order; apps that aren't installed are skipped.
    pub pinned: Vec<String>,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            size: 52,
            magnify: true,
            pinned: ["org.autark.Files", "firefox", "foot", "galley", "org.autark.Settings"]
                .map(String::from)
                .to_vec(),
        }
    }
}
