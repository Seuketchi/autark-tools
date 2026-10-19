//! `autark-brightness up|down|set <percent>`: what the brightness keys run (Wayfire's `command_light_up` and `command_light_down`). The
//! screen changes by 5 points a press, never below 5% (a screen looks off under that) or above 100, through logind's
//! `Session.SetBrightness`, which lets the person at the keyboard do it without root, as Settings' slider does.

use autark_common::backlight::Backlight;

const STEP: f64 = 5.0;

/// The level after `command`, from `percent` now, or why it isn't one.
fn target(percent: f64, command: &str, level: Option<&str>) -> Result<f64, String> {
    match (command, level) {
        ("up", None) => Ok((percent + STEP).min(100.0)),
        ("down", None) => Ok((percent - STEP).max(5.0)),
        ("set", Some(level)) => level.trim_end_matches('%').parse::<f64>().map(|level| level.clamp(5.0, 100.0)).map_err(|_| format!("“{level}” isn't a number from 5 to 100")),
        _ => Err("usage: autark-brightness up | down | set <percent>".to_owned()),
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backlight = Backlight::find().ok_or("this laptop has no screen backlight to change")?;
    let percent = target(backlight.percent(), args.first().map_or("", String::as_str), args.get(1).map(String::as_str))?;
    let value = backlight.value(percent);
    async_io::block_on(async {
        let connection = zbus::Connection::system().await.map_err(|error| format!("the system bus: {error}"))?;
        connection
            .call_method(Some("org.freedesktop.login1"), "/org/freedesktop/login1/session/auto", Some("org.freedesktop.login1.Session"), "SetBrightness", &("backlight", backlight.name.as_str(), value))
            .await
            .map(drop)
            .map_err(|error| format!("logind refused: {error}"))
    })?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("autark-brightness: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_and_down_step_five_points_within_five_and_a_hundred() {
        assert_eq!(target(50.0, "up", None), Ok(55.0));
        assert_eq!(target(50.0, "down", None), Ok(45.0));
        assert_eq!(target(98.0, "up", None), Ok(100.0));
        assert_eq!(target(100.0, "up", None), Ok(100.0));
        assert_eq!(target(7.0, "down", None), Ok(5.0));
        assert_eq!(target(5.0, "down", None), Ok(5.0));
    }

    #[test]
    fn set_takes_a_number_kept_in_range() {
        assert_eq!(target(50.0, "set", Some("30")), Ok(30.0));
        assert_eq!(target(50.0, "set", Some("30%")), Ok(30.0));
        assert_eq!(target(50.0, "set", Some("0")), Ok(5.0));
        assert_eq!(target(50.0, "set", Some("250")), Ok(100.0));
        assert!(target(50.0, "set", Some("bright")).is_err());
        assert!(target(50.0, "set", None).is_err());
        assert!(target(50.0, "sideways", None).is_err());
        assert!(target(50.0, "", None).is_err());
    }
}
