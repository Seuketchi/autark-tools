//! When it's night and how warm the screen is: `~/.config/autark/nightlight.toml`, the nights it gives, and
//! the fade in and out of each.
//!
//! ```toml
//! schedule = "sun"        # "sun" (sunset to sunrise), "custom" (from and to), or "off" (only by hand)
//! latitude = 7.07         # for "sun"; until both are set, "sun" uses from and to
//! longitude = 125.61
//! from = "22:00"
//! to = "07:00"
//! temperature = 4000      # kelvin at night; 6500 is no change
//! fade_minutes = 30
//! ```

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use serde::Deserialize;

use crate::sun::{self, Sun};

/// Daylight: the ramps are left as they are.
pub const DAY: u32 = 6500;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Sun,
    Custom,
    Off,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schedule: Kind,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub from: String,
    pub to: String,
    pub temperature: u32,
    pub fade_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { schedule: Kind::Sun, latitude: None, longitude: None, from: "22:00".into(), to: "07:00".into(), temperature: 4000, fade_minutes: 30 }
    }
}

impl Config {
    pub fn load() -> Self {
        autark_common::config::load("nightlight")
    }

    /// What the schedule comes to: "sun" without coordinates falls back to the custom times.
    pub fn kind(&self) -> Kind {
        match (self.schedule, self.latitude, self.longitude) {
            (Kind::Sun, Some(_), Some(_)) => Kind::Sun,
            (Kind::Sun, ..) => Kind::Custom,
            (kind, ..) => kind,
        }
    }

    pub fn night(&self) -> u32 {
        self.temperature.clamp(1000, DAY)
    }

    fn fade(&self) -> Duration {
        Duration::minutes(self.fade_minutes.min(180).into())
    }

    /// The nights touching the days around `now`, merged where they meet (polar nights run on).
    pub fn nights(&self, now: DateTime<Local>) -> Vec<(DateTime<Local>, DateTime<Local>)> {
        let today = now.date_naive();
        let mut nights = Vec::new();
        for offset in -1..=1 {
            let day = today + Duration::days(offset);
            match self.kind() {
                Kind::Off => {}
                Kind::Custom => {
                    let (Some(from), Some(to)) = (parse_time(&self.from), parse_time(&self.to)) else { continue };
                    let end_day = if to <= from { day + Duration::days(1) } else { day };
                    if let (Some(start), Some(end)) = (local(day, from), local(end_day, to)) {
                        nights.push((start, end));
                    }
                }
                Kind::Sun => {
                    let (latitude, longitude) = (self.latitude.unwrap_or_default(), self.longitude.unwrap_or_default());
                    let Some(midnight) = local(day, NaiveTime::MIN) else { continue };
                    let noon = |date: NaiveDate| local(date, NaiveTime::from_hms_opt(12, 0, 0).unwrap_or(NaiveTime::MIN)).map(|time| time.timestamp());
                    let (Some(noon_today), Some(noon_tomorrow)) = (noon(day), noon(day + Duration::days(1))) else { continue };
                    let at = |seconds: i64| Local.timestamp_opt(seconds, 0).single();
                    match (sun::on(noon_today, latitude, longitude), sun::on(noon_tomorrow, latitude, longitude)) {
                        (Sun::AlwaysDown, _) => nights.push((midnight, midnight + Duration::days(1))),
                        (Sun::Rises { set, .. }, Sun::Rises { rise, .. }) => {
                            if let (Some(set), Some(rise)) = (at(set), at(rise)) {
                                nights.push((set, rise));
                            }
                        }
                        (Sun::Rises { set, .. }, Sun::AlwaysDown) => {
                            if let Some(set) = at(set) {
                                nights.push((set, midnight + Duration::days(1)));
                            }
                        }
                        (Sun::AlwaysUp, _) | (Sun::Rises { .. }, Sun::AlwaysUp) => {}
                    }
                    // The night that ends today's polar night, at tomorrow's first sunrise.
                    if let (Sun::AlwaysDown, Sun::Rises { rise, .. }) = (sun::on(noon_today, latitude, longitude), sun::on(noon_tomorrow, latitude, longitude))
                        && let Some(rise) = at(rise)
                    {
                        nights.push((midnight + Duration::days(1), rise));
                    }
                }
            }
        }
        nights.sort();
        let mut merged: Vec<(DateTime<Local>, DateTime<Local>)> = Vec::new();
        for (start, end) in nights {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        merged
    }

    /// How far into night `now` is, from 0 (day) to 1 (night), fading after each night starts and before it ends.
    pub fn depth(&self, now: DateTime<Local>) -> f64 {
        let fade = self.fade().num_seconds().max(1) as f64;
        self.nights(now)
            .iter()
            .map(|(start, end)| {
                let since = (now - *start).num_seconds() as f64;
                let until = (*end - now).num_seconds() as f64;
                if since < 0.0 || until < 0.0 { 0.0 } else { (since / fade).min(until / fade).min(1.0) }
            })
            .fold(0.0, f64::max)
    }

    /// Whether the auto theme is dark at `now`: night by this schedule, or by the sun (else the set times)
    /// when night light's own schedule is off.
    pub fn theme_night(&self, now: DateTime<Local>) -> bool {
        let config = if self.schedule == Kind::Off { Config { schedule: Kind::Sun, ..self.clone() } } else { self.clone() };
        config.nights(now).iter().any(|(start, end)| *start <= now && now < *end)
    }

    /// The next time the schedule starts or finishes a fade after `now`.
    pub fn next_change(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        let fade = self.fade();
        self.nights(now).iter().flat_map(|(start, end)| [*start, *start + fade, *end - fade, *end]).filter(|time| *time > now).min()
    }
}

/// The temperature `depth` of the way from day to `night`.
pub fn temperature(depth: f64, night: u32) -> u32 {
    (f64::from(DAY) - depth.clamp(0.0, 1.0) * f64::from(DAY - night)).round() as u32
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text.trim(), "%H:%M").ok()
}

/// A local wall-clock time; the earlier one when clocks go back, and the hour after when they skip it.
fn local(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Local>> {
    let naive = date.and_time(time);
    Local.from_local_datetime(&naive).earliest().or_else(|| Local.from_local_datetime(&(naive + Duration::hours(1))).earliest())
}

/// Red, green and blue multipliers for a colour temperature, 6500 K being 1, 1, 1 (Tanner Helland's fit to
/// the black-body colours).
pub fn white_point(kelvin: u32) -> [f64; 3] {
    fn rgb(kelvin: f64) -> [f64; 3] {
        let t = kelvin / 100.0;
        let red = if t <= 66.0 { 255.0 } else { 329.698727446 * (t - 60.0).powf(-0.1332047592) };
        let green = if t <= 66.0 { 99.4708025861 * t.ln() - 161.1195681661 } else { 288.1221695283 * (t - 60.0).powf(-0.0755148492) };
        let blue = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.5177312231 * (t - 10.0).ln() - 305.0447927307
        };
        [red, green, blue].map(|value| value.clamp(0.0, 255.0) / 255.0)
    }
    let (colour, white) = (rgb(f64::from(kelvin.clamp(1000, 10000))), rgb(f64::from(DAY)));
    [0, 1, 2].map(|channel| (colour[channel] / white[channel]).min(1.0))
}

/// The gamma table `wlr-gamma-control` takes: `size` steps of red, then green, then blue.
pub fn ramps(size: usize, kelvin: u32) -> Vec<u16> {
    let point = white_point(kelvin);
    let mut table = Vec::with_capacity(size * 3);
    for multiplier in point {
        for step in 0..size {
            let level = if size > 1 { step as f64 / (size - 1) as f64 } else { 1.0 };
            table.push((level * multiplier * 65535.0).round() as u16);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Local> {
        Local.from_local_datetime(&chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()).earliest().unwrap()
    }

    fn custom() -> Config {
        Config { schedule: Kind::Custom, ..Config::default() }
    }

    #[test]
    fn custom_nights_fade_in_after_the_start_and_out_before_the_end() {
        let config = custom();
        assert_eq!(config.depth(at("2026-10-17 21:59")), 0.0);
        assert!((config.depth(at("2026-10-17 22:15")) - 0.5).abs() < 0.01);
        assert_eq!(config.depth(at("2026-10-18 02:00")), 1.0);
        assert!((config.depth(at("2026-10-18 06:45")) - 0.5).abs() < 0.01);
        assert_eq!(config.depth(at("2026-10-18 07:01")), 0.0);
        assert_eq!(config.next_change(at("2026-10-17 12:00")), Some(at("2026-10-17 22:00")));
        assert_eq!(config.next_change(at("2026-10-17 22:10")), Some(at("2026-10-17 22:30")));
        assert_eq!(temperature(0.5, 4000), 5250);
    }

    #[test]
    fn sun_without_coordinates_uses_the_custom_times_and_off_has_no_nights() {
        assert_eq!(Config::default().kind(), Kind::Custom);
        assert_eq!(Config { latitude: Some(7.0), longitude: Some(125.6), ..Config::default() }.kind(), Kind::Sun);
        let off = Config { schedule: Kind::Off, ..Config::default() };
        assert!(off.nights(at("2026-10-17 23:00")).is_empty());
        // The auto theme still has its nights, from the set times here.
        assert!(off.theme_night(at("2026-10-17 23:00")));
        assert!(!off.theme_night(at("2026-10-17 12:00")));
        assert_eq!(off.next_change(at("2026-10-17 23:00")), None);
    }

    #[test]
    fn sun_nights_run_from_sunset_to_sunrise() {
        let config = Config { latitude: Some(7.07), longitude: Some(125.61), ..Config::default() };
        let nights = config.nights(at("2026-10-17 12:00"));
        assert!(!nights.is_empty());
        for (start, end) in &nights {
            let hours = (*end - *start).num_minutes() as f64 / 60.0;
            assert!((11.0..13.5).contains(&hours), "a night near the equator lasts about 12 hours, not {hours}");
        }
        // Polar night: no gap between days, so no fade at midnight.
        let polar = Config { latitude: Some(78.2), longitude: Some(15.6), ..Config::default() };
        assert_eq!(polar.depth(at("2026-12-21 00:00")), 1.0);
        assert_eq!(polar.depth(at("2026-12-21 12:00")), 1.0);
        let summer = Config { latitude: Some(78.2), longitude: Some(15.6), ..Config::default() };
        assert_eq!(summer.depth(at("2026-06-21 00:00")), 0.0);
    }

    #[test]
    fn white_points_and_ramps() {
        assert_eq!(white_point(DAY), [1.0, 1.0, 1.0]);
        let [red, green, blue] = white_point(4000);
        assert!(red == 1.0 && green < 1.0 && blue < green);
        let table = ramps(256, DAY);
        assert_eq!((table.len(), table[0], table[255], table[256 + 255]), (768, 0, 65535, 65535));
        assert!(ramps(256, 3000)[512 + 255] < 50000);
    }
}
