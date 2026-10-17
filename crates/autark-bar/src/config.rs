//! `bar.toml`: every field is optional.

use serde::Deserialize;

use crate::clock;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bar {
    /// strftime format, as in chrono: the default reads "Fri 16 Oct  14:32".
    pub clock_format: String,
    /// Shows the battery percentage next to its icon.
    pub battery_percent: bool,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            clock_format: clock::DEFAULT_FORMAT.to_owned(),
            battery_percent: true,
        }
    }
}
