//! What the calendar shows, worked out from the events: the days of a month, and the showings on a day.
//! Nothing here touches the network or a window, so it is tested on its own.

use autark_pim::ical::{self, Calendar, Item, Kind, When};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};

/// The 42 days (six weeks) that fill a month's grid, starting on `week_start`.
pub fn month_cells(year: i32, month: u32, week_start: Weekday) -> Vec<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
    let back = (7 + first.weekday().num_days_from_monday() - week_start.num_days_from_monday()) % 7;
    let start = first - Duration::days(i64::from(back));
    (0..42).map(|n| start + Duration::days(n)).collect()
}

/// The month `delta` months from `(year, month)`.
pub fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + month as i32 - 1 + delta;
    (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
}

/// A time as the wall clock here shows it. UTC times are converted; a time in a named zone is taken as the
/// clock there (see `ical`), and a floating one is already local.
pub fn local(when: &When) -> NaiveDateTime {
    match when {
        When::Utc(at) => Utc.from_utc_datetime(at).with_timezone(&Local).naive_local(),
        other => other.naive(),
    }
}

/// A local wall time as a UTC `When`, for new events. A time that doesn't exist here (the clocks went forward)
/// is read as an hour later.
pub fn to_utc(at: NaiveDateTime) -> When {
    let moment = Local.from_local_datetime(&at).earliest().or_else(|| Local.from_local_datetime(&(at + Duration::hours(1))).earliest());
    When::Utc(moment.map_or(at, |moment| moment.with_timezone(&Utc).naive_utc()))
}

/// One event on one day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Showing {
    /// Which resource (by address) and which item in it, so the event can be edited or deleted.
    pub resource: String,
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
    pub calendar: String,
    pub color: String,
}

/// The showings that overlap `day` in the calendar's items, soonest first (all-day ones before timed ones).
/// `resource`, `calendar` and `color` say where the items came from.
pub fn showings(calendar: &Calendar, resource: &str, calendar_name: &str, color: &str, day: NaiveDate) -> Vec<Showing> {
    let from = day.and_time(NaiveTime::MIN);
    let to = from + Duration::days(1);
    // Wide enough that a UTC event shown in another zone isn't missed.
    let (search_from, search_to) = (from - Duration::days(2), to + Duration::days(2));
    let mut found = Vec::new();
    for item in calendar.items.iter().filter(|item| item.kind == Kind::Event && !item.status.eq_ignore_ascii_case("CANCELLED")) {
        if item.recurrence_id.is_some() {
            // An edited showing stands on its own.
            push(&mut found, item, item.start.clone(), item.end.clone(), resource, calendar_name, color, from, to);
            continue;
        }
        let replaced: Vec<NaiveDateTime> = calendar.items.iter().filter(|other| other.uid == item.uid).filter_map(|other| other.recurrence_id.as_ref().map(When::naive)).collect();
        for occurrence in ical::occurrences(item, search_from, search_to) {
            if replaced.contains(&occurrence.start.naive()) {
                continue;
            }
            push(&mut found, item, Some(occurrence.start), Some(occurrence.end), resource, calendar_name, color, from, to);
        }
    }
    found.sort_by(|a, b| b.all_day.cmp(&a.all_day).then(a.start.cmp(&b.start)).then(a.summary.cmp(&b.summary)));
    found
}

#[allow(clippy::too_many_arguments)]
fn push(found: &mut Vec<Showing>, item: &Item, start: Option<When>, end: Option<When>, resource: &str, calendar: &str, color: &str, from: NaiveDateTime, to: NaiveDateTime) {
    let Some(start) = start else { return };
    let all_day = start.is_date();
    let (begin, finish) = if all_day {
        (start.naive(), end.as_ref().map_or(start.naive() + Duration::days(1), When::naive))
    } else {
        let begin = local(&start);
        (begin, end.as_ref().map_or(begin, local))
    };
    // A zero-length event at midnight belongs to its own day only.
    let overlaps = begin < to && (finish > from || (finish == begin && begin >= from));
    if overlaps {
        found.push(Showing { resource: resource.to_owned(), uid: item.uid.clone(), summary: item.summary.clone(), location: item.location.clone(), start: begin, end: finish, all_day, calendar: calendar.to_owned(), color: color.to_owned() });
    }
}

/// `#RRGGBBAA` or `#RRGGBB` from a server as `#RRGGBB`, with a fallback when it's empty or odd.
pub fn colour(text: &str) -> String {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() >= 6 && hex.chars().take(6).all(|c| c.is_ascii_hexdigit()) { format!("#{}", &hex[..6]) } else { "#5B8DEF".to_owned() }
}

/// A new event from what the editor collected. `end` is the same day's end time, or the next day's if it is
/// before `start`; an all-day event ignores the times and spans `days`.
pub fn new_event(uid: &str, summary: &str, day: NaiveDate, times: Option<(NaiveTime, NaiveTime)>, days: i64) -> Item {
    let mut item = Item::new(Kind::Event, uid, summary);
    match times {
        None => {
            item.start = Some(When::Date(day));
            item.end = Some(When::Date(day + Duration::days(days.max(1))));
        }
        Some((from, to)) => {
            let start = day.and_time(from);
            let end = if to > from { day.and_time(to) } else { (day + Duration::days(1)).and_time(to) };
            item.start = Some(to_utc(start));
            item.end = Some(to_utc(end));
        }
    }
    item
}

/// `HH:MM`, or `H:MM`, or `HH`.
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    let text = text.trim();
    let (hours, minutes) = text.split_once(':').unwrap_or((text, "0"));
    NaiveTime::from_hms_opt(hours.trim().parse().ok()?, minutes.trim().parse().ok()?, 0)
}

pub fn week_start(name: &str) -> Weekday {
    match name.to_ascii_lowercase().as_str() {
        "sunday" | "sun" => Weekday::Sun,
        "saturday" | "sat" => Weekday::Sat,
        _ => Weekday::Mon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autark_pim::ical::parse;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn the_grid_covers_the_month_in_six_weeks() {
        let cells = month_cells(2026, 10, Weekday::Mon);
        assert_eq!((cells.len(), cells[0], cells[41]), (42, date(2026, 9, 28), date(2026, 11, 8)));
        assert_eq!(month_cells(2026, 10, Weekday::Sun)[0], date(2026, 9, 27));
        assert_eq!(month_cells(2026, 2, Weekday::Mon)[0], date(2026, 1, 26));
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 5, 14), (2027, 7));
    }

    #[test]
    fn days_show_timed_all_day_and_repeating_events() {
        let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nSUMMARY:Stand-up\nDTSTART:20261019T090000\nDTEND:20261019T091500\nRRULE:FREQ=DAILY;COUNT=3\nEND:VEVENT\nBEGIN:VEVENT\nUID:b\nSUMMARY:Holiday\nDTSTART;VALUE=DATE:20261020\nDTEND;VALUE=DATE:20261022\nEND:VEVENT\nBEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20261020T090000\nSUMMARY:Stand-up (moved)\nDTSTART:20261020T110000\nDTEND:20261020T113000\nEND:VEVENT\nEND:VCALENDAR\n";
        let calendar = parse(text);
        let names = |day| showings(&calendar, "r", "Work", "#fff", day).into_iter().map(|s| s.summary).collect::<Vec<_>>();
        assert_eq!(names(date(2026, 10, 19)), vec!["Stand-up"]);
        assert_eq!(names(date(2026, 10, 20)), vec!["Holiday", "Stand-up (moved)"]);
        assert_eq!(names(date(2026, 10, 21)), vec!["Holiday", "Stand-up"]);
        assert!(names(date(2026, 10, 22)).is_empty());
    }

    #[test]
    fn cancelled_events_and_the_edges_of_a_day() {
        let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:c\nSUMMARY:Gone\nSTATUS:CANCELLED\nDTSTART:20261019T090000\nEND:VEVENT\nBEGIN:VEVENT\nUID:d\nSUMMARY:Late\nDTSTART:20261019T220000\nDTEND:20261020T000000\nEND:VEVENT\nEND:VCALENDAR\n";
        let calendar = parse(text);
        assert_eq!(showings(&calendar, "r", "c", "#fff", date(2026, 10, 19)).len(), 1);
        assert!(showings(&calendar, "r", "c", "#fff", date(2026, 10, 20)).is_empty());
    }

    #[test]
    fn new_events_and_times() {
        let timed = new_event("u", "Lunch", date(2026, 10, 19), Some((parse_time("12:00").unwrap(), parse_time("13:30").unwrap())), 1);
        assert_eq!(timed.length(), Duration::minutes(90));
        let overnight = new_event("u", "Flight", date(2026, 10, 19), Some((parse_time("23:00").unwrap(), parse_time("06:00").unwrap())), 1);
        assert_eq!(overnight.length(), Duration::hours(7));
        let all_day = new_event("u", "Away", date(2026, 10, 19), None, 3);
        assert_eq!((all_day.start.clone(), all_day.length()), (Some(When::Date(date(2026, 10, 19))), Duration::days(3)));
        assert!(parse_time("9:5").is_some() && parse_time("25:00").is_none() && parse_time("noon").is_none());
        assert_eq!(colour("#0B8043FF"), "#0B8043");
        assert_eq!(colour(""), "#5B8DEF");
    }
}
