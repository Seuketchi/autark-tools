//! `bar.toml`: every field is optional.

use autark_common::layout;
use serde::Deserialize;

use crate::clock;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bar {
    /// strftime format, as in chrono: the default reads "Fri 16 Oct  14:32".
    pub clock_format: String,
    /// Shows the battery percentage next to its icon.
    pub battery_percent: bool,
    /// What shows on the left, in order: names from `autark_common::layout::START`. Leave an item out to hide it.
    pub start: Vec<String>,
    /// What shows on the right, in order: names from `autark_common::layout::END`.
    pub end: Vec<String>,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            clock_format: clock::DEFAULT_FORMAT.to_owned(),
            battery_percent: true,
            start: layout::names(&layout::START),
            end: layout::names(&layout::END),
        }
    }
}

/// The items of each side, as the bar builds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub start: Vec<&'static str>,
    pub end: Vec<&'static str>,
}

impl Bar {
    pub fn layout(&self) -> Layout {
        let side = |wanted: &[String], known: &[(&'static str, &str)]| {
            let (items, skipped) = layout::resolve(wanted, known);
            for name in skipped {
                eprintln!("bar.toml: there is no bar item called {name:?}");
            }
            items
        };
        Layout { start: side(&self.start, &layout::START), end: side(&self.end, &layout::END) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_bar_is_the_one_that_shipped() {
        let layout = Bar::default().layout();
        assert_eq!(layout.start, ["logo", "app-name"]);
        assert_eq!(layout.end, ["recording", "wifi", "volume", "battery", "kith", "clock", "notifications"]);
    }

    #[test]
    fn a_file_can_hide_and_reorder() {
        let bar: Bar = toml::from_str("end = [\"clock\", \"battery\", \"nonsense\"]\nstart = []\n").unwrap();
        let layout = bar.layout();
        assert_eq!(layout.end, ["clock", "battery"]);
        assert!(layout.start.is_empty());
        assert_eq!(bar.clock_format, clock::DEFAULT_FORMAT, "what the file leaves out is as it was");
    }
}
