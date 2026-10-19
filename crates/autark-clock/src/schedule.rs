//! Making the alarms and the timer ring when the window is shut: systemd user timers run `autark-clock ring`. An alarm
//! has a timer unit of its own in `~/.config/systemd/user`, which survives logging out; a timer is a transient one,
//! which a reboot would end anyway.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::state::{Alarm, Timer, now};

fn units_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("systemd/user")
}

/// An argument as systemd's unit files read it: in quotes, with `\`, `"` and `%` made plain.
pub fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('\n', " "))
}

/// When the alarm rings, as `OnCalendar` says it.
pub fn on_calendar(alarm: &Alarm) -> String {
    let time = format!("{:02}:{:02}:00", alarm.hour, alarm.minute);
    match (&alarm.date, alarm.days.is_empty()) {
        (Some(date), true) => format!("{date} {time}"),
        (None, true) => format!("*-*-* {time}"),
        _ => format!("{} *-*-* {time}", alarm.days.join(",")),
    }
}

pub fn service_text(alarm: &Alarm, exe: &Path) -> String {
    let label = if alarm.label.trim().is_empty() { "Alarm" } else { alarm.label.trim() };
    format!(
        "# Written by autark-clock.\n[Unit]\nDescription=Alarm: {}\n\n[Service]\nType=oneshot\nExecStart={} ring {} alarm\n",
        label.replace('\n', " ").replace('%', "%%"),
        quote(&exe.to_string_lossy()),
        quote(label)
    )
}

pub fn timer_text(alarm: &Alarm) -> String {
    format!(
        "# Written by autark-clock.\n[Unit]\nDescription=Alarm at {:02}:{:02}\n\n[Timer]\nOnCalendar={}\nAccuracySec=1s\nUnit=autark-alarm-{}.service\n\n[Install]\nWantedBy=timers.target\n",
        alarm.hour,
        alarm.minute,
        on_calendar(alarm),
        alarm.id
    )
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let output = Command::new("systemctl").arg("--user").args(args).output().map_err(|error| format!("systemctl: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&output.stderr);
    Err(text.lines().next().unwrap_or("systemctl failed").to_owned())
}

/// Writes the alarm's units and starts its timer, or stops it when it's off.
pub fn apply(alarm: &Alarm, exe: &Path) -> Result<(), String> {
    let dir = units_dir();
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let write = |name: String, text: String| std::fs::write(dir.join(&name), text).map_err(|error| format!("{name}: {error}"));
    write(format!("autark-alarm-{}.service", alarm.id), service_text(alarm, exe))?;
    write(format!("autark-alarm-{}.timer", alarm.id), timer_text(alarm))?;
    systemctl(&["daemon-reload"])?;
    let timer = format!("autark-alarm-{}.timer", alarm.id);
    if alarm.enabled { systemctl(&["enable", "--now", &timer]) } else { systemctl(&["disable", "--now", &timer]) }
}

/// Stops an alarm for good and removes its units.
pub fn remove(id: u64) -> Result<(), String> {
    let timer = format!("autark-alarm-{id}.timer");
    let _ = systemctl(&["disable", "--now", &timer]);
    for name in [format!("autark-alarm-{id}.timer"), format!("autark-alarm-{id}.service")] {
        let _ = std::fs::remove_file(units_dir().join(name));
    }
    systemctl(&["daemon-reload"])
}

/// Starts a timer of `seconds` that rings `autark-clock ring` when it's up.
pub fn start_timer(seconds: u64, exe: &Path) -> Result<Timer, String> {
    let unit = format!("autark-timer-{}", now());
    let output = Command::new("systemd-run")
        .args(["--user", "--collect", &format!("--unit={unit}"), &format!("--on-active={seconds}s"), "--timer-property=AccuracySec=1s", "--quiet", "--"])
        .arg(exe)
        .args(["ring", "Timer", "timer"])
        .output()
        .map_err(|error| format!("systemd-run: {error}"))?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        return Err(text.lines().next().unwrap_or("systemd-run failed").to_owned());
    }
    Ok(Timer { unit, end: now() + seconds, total: seconds })
}

pub fn stop_timer(unit: &str) {
    let _ = systemctl(&["stop", &format!("{unit}.timer")]);
}

/// The next date `hour:minute` comes round, as `YYYY-MM-DD`, given the local date and time now.
pub fn next_date(today: (i32, u32, u32), now_minutes: u32, hour: u8, minute: u8) -> (i32, u32, u32) {
    if u32::from(hour) * 60 + u32::from(minute) > now_minutes { today } else { add_day(today) }
}

fn add_day((year, month, day): (i32, u32, u32)) -> (i32, u32, u32) {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let length = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    match (day < length, month < 12) {
        (true, _) => (year, month, day + 1),
        (false, true) => (year, month + 1, 1),
        (false, false) => (year + 1, 1, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alarm(days: &[&str], date: Option<&str>) -> Alarm {
        Alarm { id: 5, hour: 7, minute: 5, days: days.iter().map(|day| (*day).to_owned()).collect(), date: date.map(str::to_owned), label: "Wake up".into(), enabled: true }
    }

    #[test]
    fn calendar_specs_for_once_daily_and_some_days() {
        assert_eq!(on_calendar(&alarm(&[], Some("2026-10-20"))), "2026-10-20 07:05:00");
        assert_eq!(on_calendar(&alarm(&[], None)), "*-*-* 07:05:00");
        assert_eq!(on_calendar(&alarm(&["Mon", "Wed", "Fri"], None)), "Mon,Wed,Fri *-*-* 07:05:00");
    }

    #[test]
    fn arguments_are_quoted_for_systemd() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote("say \"hi\" at 100%"), "\"say \\\"hi\\\" at 100%%\"");
        assert_eq!(quote("a\\b\nc"), "\"a\\\\b c\"");
    }

    #[test]
    fn unit_files_name_each_other() {
        let a = alarm(&["Mon"], None);
        let service = service_text(&a, Path::new("/usr/bin/autark-clock"));
        assert!(service.contains("ExecStart=\"/usr/bin/autark-clock\" ring \"Wake up\" alarm"));
        let timer = timer_text(&a);
        assert!(timer.contains("Unit=autark-alarm-5.service") && timer.contains("OnCalendar=Mon *-*-* 07:05:00") && timer.contains("WantedBy=timers.target"));
    }

    /// Whether a user systemd is there to try things on.
    fn systemd() -> bool {
        Command::new("systemctl").args(["--user", "show-environment"]).output().is_ok_and(|output| output.status.success())
    }

    fn active(unit: &str) -> bool {
        Command::new("systemctl").args(["--user", "is-active", unit]).output().is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "active")
    }

    #[test]
    fn a_timer_is_a_systemd_timer_until_it_is_stopped() {
        if !systemd() {
            return;
        }
        // `true` stands in for the ringing, so nothing sounds.
        let timer = start_timer(3600, Path::new("/usr/bin/true")).unwrap();
        assert!(active(&format!("{}.timer", timer.unit)), "{timer:?}");
        assert_eq!(timer.total, 3600);
        stop_timer(&timer.unit);
        assert!(!active(&format!("{}.timer", timer.unit)));
    }

    #[test]
    fn an_alarm_has_units_that_come_and_go() {
        if !systemd() {
            return;
        }
        let id = 4_242_000_000_000 + u64::from(std::process::id());
        let mut alarm = Alarm { id, hour: 3, minute: 33, days: vec!["Mon".into()], date: None, label: "Test, \"quoted\" 100%".into(), enabled: true };
        apply(&alarm, Path::new("/usr/bin/true")).unwrap();
        let timer = format!("autark-alarm-{id}.timer");
        assert!(units_dir().join(&timer).exists() && units_dir().join(format!("autark-alarm-{id}.service")).exists());
        assert!(active(&timer));
        alarm.enabled = false;
        apply(&alarm, Path::new("/usr/bin/true")).unwrap();
        assert!(!active(&timer));
        remove(id).unwrap();
        assert!(!units_dir().join(&timer).exists() && !units_dir().join(format!("autark-alarm-{id}.service")).exists());
    }

    #[test]
    fn a_one_time_alarm_rings_today_if_there_is_time_and_tomorrow_if_not() {
        assert_eq!(next_date((2026, 10, 19), 6 * 60, 7, 5), (2026, 10, 19));
        assert_eq!(next_date((2026, 10, 19), 8 * 60, 7, 5), (2026, 10, 20));
        assert_eq!(next_date((2026, 10, 31), 23 * 60, 7, 5), (2026, 11, 1));
        assert_eq!(next_date((2026, 12, 31), 23 * 60, 7, 5), (2027, 1, 1));
        assert_eq!(next_date((2028, 2, 28), 23 * 60, 7, 5), (2028, 2, 29));
        assert_eq!(next_date((2027, 2, 28), 23 * 60, 7, 5), (2027, 3, 1));
    }
}
