//! Kith's calendar: what's on it, read from an iCalendar address you gave Kith (Google Calendar's "secret address in
//! iCal format", Fastmail's and Outlook's subscribe links work the same way). Read-only, no sign-in, no OAuth client:
//! the address is the key, so it lives in the keyring (`application = autark-ai`, `key = calendar`), or in
//! `KITH_CALENDAR_URL`, and is never logged or shown.
//!
//! The events stay on this laptop. A question about the calendar is answered from the events by code, not by the
//! model, so it can't invent a meeting; the loop closes the web after a calendar read, as after any private read.

use std::time::Duration as Wait;

use autark_pim::ical::{self, Kind, When};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

/// The most a feed may be, so a wrong address can't fill memory.
const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// The most days a question may ask about.
const MAX_DAYS: i64 = 31;

/// One event on one day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Showing {
    pub summary: String,
    pub location: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
}

/// The feed's address: `KITH_CALENDAR_URL`, else the keyring's. `webcal://` is `https://`.
pub async fn url(connection: &zbus::Connection) -> Result<String, String> {
    let found = match std::env::var("KITH_CALENDAR_URL").ok().filter(|url| !url.is_empty()) {
        Some(url) => url,
        None => crate::jev::stored(connection, "calendar").await.map_err(|err| format!("the calendar address: {err}"))?,
    };
    let found = found.trim();
    let address = found.strip_prefix("webcal://").map_or_else(|| found.to_owned(), |rest| format!("https://{rest}"));
    crate::web::allowed(&address).map_err(|_| "the calendar address isn't an https:// (or webcal://) address".to_owned())?;
    Ok(address)
}

/// The feed's text. Errors never carry the address, which is the key.
pub fn fetch(address: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Wait::from_secs(20))).https_only(true).http_status_as_error(false).build().into();
    let response = agent.get(address).call().map_err(|_| "couldn't reach the calendar".to_owned())?;
    if !response.status().is_success() {
        return Err(format!("the calendar answered {}", response.status()));
    }
    let text = response.into_body().with_config().limit(MAX_BYTES).read_to_string().map_err(|_| "couldn't read the calendar".to_owned())?;
    if !text.contains("BEGIN:VCALENDAR") {
        return Err("that address isn't an iCalendar feed".to_owned());
    }
    Ok(text)
}

/// A time as the wall clock here shows it: UTC converted, a named zone taken as the clock there, as Calendar does.
fn local(when: &When) -> NaiveDateTime {
    match when {
        When::Utc(at) => Utc.from_utc_datetime(at).with_timezone(&Local).naive_local(),
        other => other.naive(),
    }
}

/// The events on the `days` days from `first`, soonest first, all-day ones leading each day.
pub fn agenda(feed: &str, first: NaiveDate, days: i64) -> Vec<Showing> {
    let calendar = ical::parse(feed);
    let days = days.clamp(1, MAX_DAYS);
    let (from, to) = (first.and_time(NaiveTime::MIN), first.and_time(NaiveTime::MIN) + Duration::days(days));
    // Wide enough that a UTC event shown in another zone isn't missed.
    let (search_from, search_to) = (from - Duration::days(2), to + Duration::days(2));
    let mut found = Vec::new();
    for item in calendar.items.iter().filter(|item| item.kind == Kind::Event && !item.status.eq_ignore_ascii_case("CANCELLED")) {
        if item.recurrence_id.is_some() {
            // An edited showing stands on its own.
            push(&mut found, item, item.start.clone(), item.end.clone(), from, to);
            continue;
        }
        let replaced: Vec<NaiveDateTime> = calendar.items.iter().filter(|other| other.uid == item.uid).filter_map(|other| other.recurrence_id.as_ref().map(When::naive)).collect();
        for occurrence in ical::occurrences(item, search_from, search_to) {
            if !replaced.contains(&occurrence.start.naive()) {
                push(&mut found, item, Some(occurrence.start), Some(occurrence.end), from, to);
            }
        }
    }
    found.sort_by(|a, b| a.start.date().cmp(&b.start.date()).then(b.all_day.cmp(&a.all_day)).then(a.start.cmp(&b.start)).then(a.summary.cmp(&b.summary)));
    found
}

fn push(found: &mut Vec<Showing>, item: &ical::Item, start: Option<When>, end: Option<When>, from: NaiveDateTime, to: NaiveDateTime) {
    let Some(start) = start else { return };
    let all_day = start.is_date();
    let (begin, finish) = if all_day {
        (start.naive(), end.as_ref().map_or(start.naive() + Duration::days(1), When::naive))
    } else {
        let begin = local(&start);
        (begin, end.as_ref().map_or(begin, local))
    };
    // A zero-length event belongs to its own day only.
    if begin < to && (finish > from || (finish == begin && begin >= from)) {
        found.push(Showing { summary: item.summary.trim().to_owned(), location: item.location.trim().to_owned(), start: begin, end: finish, all_day });
    }
}

/// Which days a question is about: `(first day, how many, what to call it)`. "Tomorrow" is tomorrow, "this week" the
/// next seven days, "next week" the seven from next Monday; anything else is today and the next two days.
pub fn window(question: &str, today: NaiveDate) -> (NaiveDate, i64, &'static str) {
    let text = question.to_lowercase();
    if text.contains("next week") {
        let ahead = 7 - i64::from(today.weekday().num_days_from_monday());
        (today + Duration::days(ahead), 7, "next week")
    } else if text.contains("week") {
        (today, 7, "the next seven days")
    } else if text.contains("tomorrow") {
        (today + Duration::days(1), 1, "tomorrow")
    } else if text.contains("today") || text.contains("tonight") {
        (today, 1, "today")
    } else {
        (today, 3, "the next three days")
    }
}

/// The agenda in words, a heading per day with its events; days with nothing say so.
pub fn describe(showings: &[Showing], first: NaiveDate, days: i64, today: NaiveDate, label: &str) -> String {
    let mut out = vec![format!("Your calendar for {label}:")];
    for offset in 0..days.clamp(1, MAX_DAYS) {
        let date = first + Duration::days(offset);
        let name = match (date - today).num_days() {
            0 => "Today".to_owned(),
            1 => "Tomorrow".to_owned(),
            _ => date.format("%A").to_string(),
        };
        out.push(format!("\n{name}, {}", date.format("%-d %B")));
        let mut any = false;
        for showing in showings.iter().filter(|showing| showing.start.date() == date || (showing.all_day && showing.start.date() <= date && showing.end.date() > date)) {
            any = true;
            let place = if showing.location.is_empty() { String::new() } else { format!(" ({})", showing.location) };
            let title = if showing.summary.is_empty() { "(no title)" } else { showing.summary.as_str() };
            out.push(if showing.all_day { format!("  all day  {title}{place}") } else { format!("  {}  {title}{place}", showing.start.format("%H:%M")) });
        }
        if !any {
            out.push("  nothing".to_owned());
        }
    }
    out.join("\n")
}

/// Answers a question from the feed: fetch, expand, describe. All-day events spanning several days show on each.
pub fn answer(address: &str, question: &str) -> Result<String, String> {
    let feed = fetch(address)?;
    let today = Local::now().date_naive();
    let (first, days, label) = window(question, today);
    Ok(describe(&agenda(&feed, first, days), first, days, today, label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Standup\r\nDTSTART:20261020T090000\r\nDTEND:20261020T093000\r\nLOCATION:Room 2\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Rent due\r\nDTSTART;VALUE=DATE:20261020\r\nDTEND;VALUE=DATE:20261021\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Gym\r\nDTSTART:20261012T180000\r\nDTEND:20261012T190000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:d\r\nSUMMARY:Cancelled thing\r\nSTATUS:CANCELLED\r\nDTSTART:20261020T120000\r\nDTEND:20261020T130000\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:e\r\nSUMMARY:Trip\r\nDTSTART;VALUE=DATE:20261021\r\nDTEND;VALUE=DATE:20261024\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn it_lists_the_days_events_with_repeats_and_all_day_ones_first() {
        let events = agenda(FEED, day(19), 3);
        let titles: Vec<(&str, u32)> = events.iter().map(|showing| (showing.summary.as_str(), chrono::Datelike::day(&showing.start))).collect();
        assert_eq!(titles, [("Gym", 19), ("Rent due", 20), ("Standup", 20), ("Trip", 21)], "{events:?}");
        assert!(!events.iter().any(|showing| showing.summary == "Cancelled thing"));
        assert_eq!(events[2].location, "Room 2");
    }

    #[test]
    fn it_says_what_is_on_and_what_is_not() {
        let events = agenda(FEED, day(19), 3);
        let text = describe(&events, day(19), 3, day(19), "the next three days");
        assert_eq!(
            text,
            "Your calendar for the next three days:\n\nToday, 19 October\n  18:00  Gym\n\nTomorrow, 20 October\n  all day  Rent due\n  09:00  Standup (Room 2)\n\nWednesday, 21 October\n  all day  Trip"
        );
        let empty = describe(&agenda(FEED, day(25), 1), day(25), 1, day(19), "that day");
        assert!(empty.ends_with("Sunday, 25 October\n  nothing"), "{empty}");
        // A three-day all-day event shows on each of its days.
        let trip = describe(&agenda(FEED, day(22), 2), day(22), 2, day(19), "x");
        assert_eq!(trip.matches("Trip").count(), 2, "{trip}");
    }

    #[test]
    fn a_question_picks_its_days() {
        let monday = day(19);
        assert_eq!(window("what's on my calendar tomorrow", monday), (day(20), 1, "tomorrow"));
        assert_eq!(window("whats on my calendar today?", monday), (monday, 1, "today"));
        assert_eq!(window("what do I have this week", monday), (monday, 7, "the next seven days"));
        assert_eq!(window("what's on next week", monday), (day(26), 7, "next week"));
        assert_eq!(window("whats on my calendar", monday), (monday, 3, "the next three days"));
        assert_eq!(window("anything on next week", day(25)).0, day(26), "next week from a Sunday starts tomorrow");
    }

    #[test]
    fn utc_events_are_shown_on_the_wall_clock_here() {
        let feed = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u\r\nSUMMARY:Call\r\nDTSTART:20261020T100000Z\r\nDTEND:20261020T110000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let shown = agenda(feed, day(19), 3);
        assert_eq!(shown.len(), 1);
        let expected = Utc.with_ymd_and_hms(2026, 10, 20, 10, 0, 0).unwrap().with_timezone(&Local).naive_local();
        assert_eq!(shown[0].start, expected);
    }

    #[test]
    fn the_feed_must_be_a_calendar_and_its_address_stays_secret() {
        assert!(fetch("http://example.com/x.ics").is_err());
        let err = fetch("https://127.0.0.1:9/private-key-token.ics").unwrap_err();
        assert!(!err.contains("private-key-token"), "{err}");
    }
}
