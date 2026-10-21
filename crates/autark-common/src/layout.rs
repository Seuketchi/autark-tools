//! What the top bar can show, by name, and the dock's default apps: `bar.toml`'s `start` and `end` lists name items from these, in the order they appear.
//! A name this doesn't know is skipped (the bar says so on stderr), so a typo or an item from a newer Autark never breaks it.

/// The items on the left of the bar, in their default order, with the name Settings gives them.
pub const START: [(&str, &str); 2] = [("logo", "Autark menu"), ("app-name", "Name of the app in front")];

/// The items on the right, in their default order.
pub const END: [(&str, &str); 7] = [
    ("recording", "Recording"),
    ("wifi", "Wi-Fi"),
    ("volume", "Volume"),
    ("battery", "Battery"),
    ("kith", "Kith"),
    ("clock", "Clock"),
    ("notifications", "Notification centre"),
];

/// `known`'s names as the default list for a side.
pub fn names(known: &[(&str, &str)]) -> Vec<String> {
    known.iter().map(|(name, _)| (*name).to_owned()).collect()
}

/// The default pinned apps as a list of names.
pub fn names_of(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// The names in `wanted` that `known` has, each once, in the order asked; and the ones skipped, for the log.
pub fn resolve(wanted: &[String], known: &[(&'static str, &str)]) -> (Vec<&'static str>, Vec<String>) {
    let mut items: Vec<&'static str> = Vec::new();
    let mut skipped = Vec::new();
    for name in wanted {
        match known.iter().find(|(candidate, _)| candidate == name) {
            Some((found, _)) if !items.contains(found) => items.push(found),
            Some(_) => {}
            None => skipped.push(name.clone()),
        }
    }
    (items, skipped)
}

/// The apps the dock has pinned until `dock.toml` says otherwise, by desktop file name.
pub const PINNED: [&str; 5] = ["autark-files", "firefox", "org.autark.Terminal", "galley", "autark-settings"];

/// What `bar.toml`'s clock can read as, and the format each is: the first is what Autark ships.
pub const CLOCKS: [(&str, &str, &str); 5] = [
    ("default", "Day, date and time", "%a %-d %b  %H:%M"),
    ("twelve", "Day, date and 12-hour time", "%a %-d %b  %-I:%M %p"),
    ("time", "Time only", "%H:%M"),
    ("seconds", "With seconds", "%a %-d %b  %H:%M:%S"),
    ("full", "With the year", "%a %-d %b %Y  %H:%M"),
];

/// The preset whose format is `format`, if it is one; `"custom"` otherwise.
pub fn clock_preset(format: &str) -> &'static str {
    CLOCKS.iter().find(|(_, _, candidate)| *candidate == format).map_or("custom", |(id, ..)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_unchanged_bar_shows_everything_in_the_shipped_order() {
        assert_eq!(resolve(&names(&END), &END).0, ["recording", "wifi", "volume", "battery", "kith", "clock", "notifications"]);
        assert_eq!(resolve(&names(&START), &START).0, ["logo", "app-name"]);
    }

    #[test]
    fn items_come_in_the_order_asked_and_unknown_or_repeated_ones_are_skipped() {
        let (items, skipped) = resolve(&list(&["clock", "wifi", "clock", "teleporter"]), &END);
        assert_eq!(items, ["clock", "wifi"]);
        assert_eq!(skipped, ["teleporter"]);
        assert!(resolve(&[], &END).0.is_empty(), "an empty list hides the lot");
    }

    #[test]
    fn the_clock_presets_are_valid_and_distinct() {
        let formats: std::collections::BTreeSet<&str> = CLOCKS.iter().map(|(.., format)| *format).collect();
        assert_eq!(formats.len(), CLOCKS.len());
        for (id, _, format) in CLOCKS {
            assert_eq!(clock_preset(format), id);
            assert!(!format.is_empty());
        }
        assert_eq!(clock_preset("%Y"), "custom");
        assert_eq!(CLOCKS[0].2, "%a %-d %b  %H:%M", "the default is what the bar uses without a setting");
    }
}
