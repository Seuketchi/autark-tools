//! The laptop's backlight in `/sys/class/backlight`. Reading it needs nothing; setting it goes through
//! logind's `Session.SetBrightness`, which lets the session's user change it without root.

#[derive(Clone, Debug)]
pub struct Backlight {
    /// The device's name under `/sys/class/backlight`, as logind wants it.
    pub name: String,
    pub max: u32,
}

impl Backlight {
    /// The firmware's interface if there is one, as it's the one that works best, then the platform's, then raw.
    pub fn find() -> Option<Self> {
        let mut found: Vec<(u8, Self)> = std::fs::read_dir("/sys/class/backlight")
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let read = |file: &str| std::fs::read_to_string(path.join(file)).map(|text| text.trim().to_owned()).ok();
                let max = read("max_brightness")?.parse().ok().filter(|max| *max > 0)?;
                let rank = match read("type").as_deref() {
                    Some("firmware") => 0,
                    Some("platform") => 1,
                    _ => 2,
                };
                Some((rank, Self { name: entry.file_name().to_string_lossy().into_owned(), max }))
            })
            .collect();
        found.sort_by_key(|(rank, _)| *rank);
        found.into_iter().next().map(|(_, backlight)| backlight)
    }

    /// Now, from 5 to 100: below 5% a screen looks off.
    pub fn percent(&self) -> f64 {
        let now: f64 = std::fs::read_to_string(format!("/sys/class/backlight/{}/brightness", self.name)).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(f64::from(self.max));
        (now / f64::from(self.max) * 100.0).clamp(5.0, 100.0)
    }

    /// The raw value for a percentage, never 0.
    pub fn value(&self, percent: f64) -> u32 {
        ((f64::from(self.max) * percent / 100.0).round() as u32).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_become_raw_values() {
        let backlight = Backlight { name: "intel_backlight".into(), max: 19200 };
        assert_eq!(backlight.value(30.0), 5760);
        assert_eq!(backlight.value(0.0), 1);
        assert_eq!(backlight.value(100.0), 19200);
    }
}
