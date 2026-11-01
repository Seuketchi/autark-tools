//! The payday reminder: what a payday leaves you, and what falls due tomorrow, as a desktop notification.
//! `autark-budget remind` says it when there is something to say and stays quiet otherwise, so a daily timer can run it.

use autark_finance::payday::Period;
use chrono::{Duration, NaiveDate};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

#[derive(Debug, PartialEq)]
pub struct Reminder {
    pub title: String,
    pub body: String,
}

/// What to say today about `period` (the one `today` is in), or nothing. `always` says the payday summary any day.
pub fn reminder(period: &Period, today: NaiveDate, always: bool) -> Option<Reminder> {
    let unpaid_on = |day: NaiveDate| -> Vec<String> { period.lines.iter().filter(|line| !line.paid && line.due == day).map(|line| format!("{} {}", line.name, line.amount)).collect() };

    // On payday: what it leaves and what is still to pay out of it.
    if period.start == today || always {
        let things: Vec<String> = period.lines.iter().filter(|line| !line.paid).map(|line| format!("{} {}", line.name, line.amount)).collect();
        let (title, mut body) = if period.is_short() {
            (format!("Payday: {} short", period.left.abs()), "More falls due than this paycheck brings.".to_string())
        } else {
            (format!("Payday: {} to spend", period.left), format!("That is {} a day for {} days.", period.per_day(), period.days()))
        };
        if !things.is_empty() {
            body.push_str(&format!(" To pay: {}.", things.join(", ")));
        }
        return Some(Reminder { title, body });
    }

    // Other days: only what is due today or tomorrow and not yet paid.
    let (now, tomorrow) = (unpaid_on(today), unpaid_on(today + Duration::days(1)));
    let mut parts = Vec::new();
    if !now.is_empty() {
        parts.push(format!("Due today: {}.", now.join(", ")));
    }
    if !tomorrow.is_empty() {
        parts.push(format!("Due tomorrow: {}.", tomorrow.join(", ")));
    }
    let title = if !now.is_empty() { "Due today" } else if !tomorrow.is_empty() { "Due tomorrow" } else { return None };
    Some(Reminder { title: title.to_string(), body: parts.join(" ") })
}

/// Shows it through the desktop's notification service (`autark-notifyd` here).
pub fn send(reminder: &Reminder) -> Result<(), String> {
    let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).map_err(|e| format!("no session bus: {e}"))?;
    let hints: std::collections::HashMap<String, glib::Variant> = std::collections::HashMap::new();
    let call = ("Budget", 0u32, "autark-budget", reminder.title.as_str(), reminder.body.as_str(), Vec::<String>::new(), hints, -1i32).to_variant();
    connection
        .call_sync(Some("org.freedesktop.Notifications"), "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "Notify", Some(&call), None, gio::DBusCallFlags::NONE, 5000, gio::Cancellable::NONE)
        .map(|_| ())
        .map_err(|e| format!("could not show the notification: {e}"))
}

#[cfg(test)]
mod tests {
    use autark_finance::obligation::Confidence;
    use autark_finance::money::Money;
    use autark_finance::payday::Line;
    use autark_finance::plan::Kind;

    use super::*;

    fn day(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn line(name: &str, pesos: i64, due: NaiveDate, paid: bool) -> Line {
        Line { name: name.into(), kind: Kind::Bill, amount: Money::from_pesos(pesos), due, confidence: Confidence::Firm, paid }
    }

    fn period(lines: Vec<Line>, income: i64) -> Period {
        let due: Money = lines.iter().map(|l| l.amount).sum();
        Period { start: day(15), end: day(30), income: Money::from_pesos(income), left: Money::from_pesos(income) - due, due, lines, current: true }
    }

    #[test]
    fn on_payday_it_says_what_is_left_and_what_to_pay() {
        let p = period(vec![line("Rent", 4000, day(20), false), line("Laundry", 170, day(15), true)], 12_000);
        let r = reminder(&p, day(15), false).unwrap();
        assert_eq!(r.title, "Payday: ₱7,830 to spend");
        assert!(r.body.contains("To pay: Rent ₱4,000."), "{}", r.body);
        assert!(!r.body.contains("Laundry"), "paid lines are not asked for again: {}", r.body);
    }

    #[test]
    fn a_short_payday_says_so() {
        let p = period(vec![line("Rent", 4000, day(20), false)], 3_000);
        assert_eq!(reminder(&p, day(15), false).unwrap().title, "Payday: ₱1,000 short");
    }

    #[test]
    fn the_day_before_a_bill_it_warns_and_other_days_it_is_quiet() {
        let p = period(vec![line("Rent", 4000, day(20), false)], 12_000);
        let r = reminder(&p, day(19), false).unwrap();
        assert_eq!((r.title.as_str(), r.body.as_str()), ("Due tomorrow", "Due tomorrow: Rent ₱4,000."));
        assert_eq!(reminder(&p, day(17), false), None);
        let paid = period(vec![line("Rent", 4000, day(20), true)], 12_000);
        assert_eq!(reminder(&paid, day(19), false), None, "nothing to warn about once it is paid");
    }

    #[test]
    fn the_day_a_bill_is_due_it_says_so() {
        let p = period(vec![line("Rent", 4000, day(20), false)], 12_000);
        let r = reminder(&p, day(20), false).unwrap();
        assert_eq!(r.title, "Due today");
        assert!(r.body.contains("Rent ₱4,000"));
    }

    #[test]
    fn always_gives_the_payday_summary_on_any_day() {
        let p = period(vec![line("Rent", 4000, day(20), false)], 12_000);
        assert!(reminder(&p, day(17), true).unwrap().title.starts_with("Payday"));
    }
}
