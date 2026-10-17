//! Sunrise and sunset from latitude and longitude, worked out locally (the sunrise equation, as NOAA and
//! Wikipedia give it), so the location never leaves the laptop. Good to a minute or two, plenty for a fade.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sun {
    /// Unix seconds of sunrise and sunset on the day asked about.
    Rises { rise: i64, set: i64 },
    /// Polar day: the sun never sets.
    AlwaysUp,
    /// Polar night: the sun never rises.
    AlwaysDown,
}

/// The sun on the day whose solar noon falls nearest `noon` (unix seconds) at `latitude`, `longitude` (degrees,
/// east and north positive).
pub fn on(noon: i64, latitude: f64, longitude: f64) -> Sun {
    let julian = noon as f64 / 86400.0 + 2440587.5;
    let day = (julian - 2451545.0 + 0.0008 + longitude / 360.0).round();
    let mean_noon = day - longitude / 360.0;
    let anomaly = (357.5291 + 0.98560028 * mean_noon).rem_euclid(360.0).to_radians();
    let centre = 1.9148 * anomaly.sin() + 0.02 * (2.0 * anomaly).sin() + 0.0003 * (3.0 * anomaly).sin();
    let ecliptic = (anomaly.to_degrees() + centre + 180.0 + 102.9372).rem_euclid(360.0).to_radians();
    let transit = 2451545.0 + mean_noon + 0.0053 * anomaly.sin() - 0.0069 * (2.0 * ecliptic).sin();
    let declination = (ecliptic.sin() * 23.4397f64.to_radians().sin()).asin();
    let latitude = latitude.to_radians();
    let hour_angle = ((-0.833f64).to_radians().sin() - latitude.sin() * declination.sin()) / (latitude.cos() * declination.cos());
    if hour_angle > 1.0 {
        return Sun::AlwaysDown;
    }
    if hour_angle < -1.0 {
        return Sun::AlwaysUp;
    }
    let half = hour_angle.acos().to_degrees() / 360.0;
    let unix = |julian: f64| ((julian - 2440587.5) * 86400.0).round() as i64;
    Sun::Rises { rise: unix(transit - half), set: unix(transit + half) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix seconds of a UTC date and time.
    fn utc(y: i64, m: i64, d: i64, h: i64, min: i64) -> i64 {
        // Days from civil, after Howard Hinnant.
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        (era * 146097 + doe - 719468) * 86400 + h * 3600 + min * 60
    }

    fn near(actual: i64, expected: i64) -> bool {
        (actual - expected).abs() <= 3 * 60
    }

    #[test]
    fn new_york_midwinter() {
        // 07:17 and 16:32 Eastern Standard Time (UTC-5).
        let Sun::Rises { rise, set } = on(utc(2026, 12, 21, 17, 0), 40.71, -74.01) else { panic!() };
        assert!(near(rise, utc(2026, 12, 21, 12, 17)), "{}", (rise - utc(2026, 12, 21, 12, 17)) / 60);
        assert!(near(set, utc(2026, 12, 21, 21, 32)), "{}", (set - utc(2026, 12, 21, 21, 32)) / 60);
    }

    #[test]
    fn london_midsummer() {
        // 04:43 and 21:21 British Summer Time (UTC+1).
        let Sun::Rises { rise, set } = on(utc(2026, 6, 21, 12, 0), 51.5, -0.13) else { panic!() };
        assert!(near(rise, utc(2026, 6, 21, 3, 43)), "{}", (rise - utc(2026, 6, 21, 3, 43)) / 60);
        assert!(near(set, utc(2026, 6, 21, 20, 21)), "{}", (set - utc(2026, 6, 21, 20, 21)) / 60);
    }

    #[test]
    fn polar_day_and_night() {
        assert_eq!(on(utc(2026, 6, 21, 12, 0), 69.65, 18.96), Sun::AlwaysUp);
        assert_eq!(on(utc(2026, 12, 21, 12, 0), 69.65, 18.96), Sun::AlwaysDown);
    }
}
