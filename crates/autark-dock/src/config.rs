//! `dock.toml`: every field is optional.

use serde::Deserialize;

#[derive(Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dock {
    /// Icon size in pixels, from 32 to 80.
    pub size: i32,
    /// Icons grow to half again their size under the pointer.
    pub magnify: bool,
    /// A dot beside each running app.
    pub dots: bool,
    pub position: Position,
    /// Hidden until the pointer reaches its edge of the screen; windows then use the whole screen.
    pub autohide: bool,
    /// Desktop file names, in order; apps that aren't installed are skipped.
    pub pinned: Vec<String>,
}

#[derive(Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Left,
    #[default]
    Bottom,
    Right,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            size: 52,
            magnify: true,
            dots: true,
            position: Position::Bottom,
            autohide: false,
            pinned: ["autark-files", "firefox", "org.autark.Terminal", "galley", "autark-settings"].map(String::from).to_vec(),
        }
    }
}

impl Dock {
    pub fn load() -> Self {
        let mut config: Self = autark_common::config::load("dock");
        config.size = config.size.clamp(32, 80);
        config
    }
}
