//! iCalendar (RFC 5545): the events and to-dos a CalDAV server stores, read and written without losing what
//! this code doesn't understand (alarms, attendees, vendor properties and time zone definitions come back out
//! as they went in), and recurring events expanded for the days a calendar shows.
//!
//! Times with a `TZID` are kept as the wall time in that zone and shown as written: there is no time zone
//! database here, so a calendar in another zone than yours reads as that zone's clock.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

/// A date or a moment, as an iCalendar property writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum When {
    /// An all-day date (`VALUE=DATE`).
    Date(NaiveDate),
    /// A wall time that belongs to no zone.
    Floating(NaiveDateTime),
    /// UTC (`…Z`).
    Utc(NaiveDateTime),
    /// A wall time in the named zone (`TZID=…`).
    Zoned(NaiveDateTime, String),
}

impl When {
    /// The date and time as written, an all-day date being midnight.
    pub fn naive(&self) -> NaiveDateTime {
        match self {
            When::Date(date) => date.and_time(NaiveTime::MIN),
            When::Floating(at) | When::Utc(at) | When::Zoned(at, _) => *at,
        }
    }

    pub fn is_date(&self) -> bool {
        matches!(self, When::Date(_))
    }

    /// The same kind of time, moved to `at`.
    pub fn moved(&self, at: NaiveDateTime) -> When {
        match self {
            When::Date(_) => When::Date(at.date()),
            When::Floating(_) => When::Floating(at),
            When::Utc(_) => When::Utc(at),
            When::Zoned(_, zone) => When::Zoned(at, zone.clone()),
        }
    }

    fn parse(value: &str, params: &[(String, String)]) -> Option<When> {
        let value = value.trim();
        let param = |name: &str| params.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str());
        if param("VALUE").is_some_and(|kind| kind.eq_ignore_ascii_case("DATE")) || (value.len() == 8 && !value.contains('T')) {
            return NaiveDate::parse_from_str(value, "%Y%m%d").ok().map(When::Date);
        }
        let utc = value.ends_with('Z');
        let at = NaiveDateTime::parse_from_str(value.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok()?;
        Some(match (utc, param("TZID")) {
            (true, _) => When::Utc(at),
            (false, Some(zone)) => When::Zoned(at, zone.to_owned()),
            (false, None) => When::Floating(at),
        })
    }

    /// The property's parameters and value, as written after its name.
    fn write(&self) -> (String, String) {
        match self {
            When::Date(date) => (";VALUE=DATE".to_owned(), date.format("%Y%m%d").to_string()),
            When::Floating(at) => (String::new(), at.format("%Y%m%dT%H%M%S").to_string()),
            When::Utc(at) => (String::new(), at.format("%Y%m%dT%H%M%SZ").to_string()),
            When::Zoned(at, zone) => (format!(";TZID={zone}"), at.format("%Y%m%dT%H%M%S").to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Event,
    Todo,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Event => "VEVENT",
            Kind::Todo => "VTODO",
        }
    }
}

/// One event or to-do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub start: Option<When>,
    pub end: Option<When>,
    /// A to-do's due time.
    pub due: Option<When>,
    /// `CONFIRMED`, `CANCELLED`, `NEEDS-ACTION`, `COMPLETED` and so on, as written.
    pub status: String,
    pub completed: Option<When>,
    /// The recurrence rule's value, such as `FREQ=WEEKLY;BYDAY=MO,WE`.
    pub rrule: Option<String>,
    pub exdates: Vec<When>,
    /// Set on an edited occurrence of a recurring event: which one it replaces.
    pub recurrence_id: Option<When>,
    /// Lines this code doesn't read (alarms, attendees, vendor properties), kept for writing back.
    pub extra: Vec<String>,
}

impl Item {
    pub fn new(kind: Kind, uid: &str, summary: &str) -> Item {
        Item {
            kind,
            uid: uid.to_owned(),
            summary: summary.to_owned(),
            description: String::new(),
            location: String::new(),
            start: None,
            end: None,
            due: None,
            status: String::new(),
            completed: None,
            rrule: None,
            exdates: Vec::new(),
            recurrence_id: None,
            extra: Vec::new(),
        }
    }

    /// How long an event lasts: to its end, else a day for an all-day event and nothing otherwise.
    pub fn length(&self) -> Duration {
        match (&self.start, &self.end) {
            (Some(start), Some(end)) => end.naive() - start.naive(),
            (Some(When::Date(_)), None) => Duration::days(1),
            _ => Duration::zero(),
        }
    }

    pub fn is_done(&self) -> bool {
        self.completed.is_some() || self.status.eq_ignore_ascii_case("COMPLETED")
    }
}

/// A whole `VCALENDAR`: its events and to-dos, and the time zone definitions they refer to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calendar {
    /// Lines between the calendar's own properties and its items that aren't items (`VTIMEZONE` blocks).
    pub prelude: Vec<String>,
    pub items: Vec<Item>,
}

/// A line of the text, unfolded: its name, parameters and value.
struct Line {
    name: String,
    params: Vec<(String, String)>,
    value: String,
    raw: String,
}

fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
        } else if !line.is_empty() {
            lines.push(line.to_owned());
        }
    }
    lines
}

fn split(raw: &str) -> Option<Line> {
    // The first colon outside quotes ends the name and parameters.
    let mut quoted = false;
    let mut colon = None;
    for (at, c) in raw.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(at);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&raw[..colon], &raw[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts.filter_map(|part| part.split_once('=')).map(|(key, value)| (key.to_ascii_uppercase(), value.trim_matches('"').to_owned())).collect();
    Some(Line { name, params, value: value.to_owned(), raw: raw.to_owned() })
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Reads the calendar in `text`. Items that aren't events or to-dos (journals, free/busy) are ignored.
pub fn parse(text: &str) -> Calendar {
    let mut calendar = Calendar::default();
    let mut current: Option<Item> = None;
    // Raw lines of a nested block (an alarm) inside the current item, or a zone definition outside any.
    let mut nested: Vec<String> = Vec::new();
    let mut depth = 0usize;
    for raw in unfold(text) {
        let upper = raw.to_ascii_uppercase();
        if depth > 0 {
            nested.push(raw.clone());
            if upper.starts_with("BEGIN:") {
                depth += 1;
            } else if upper.starts_with("END:") {
                depth -= 1;
                if depth == 0 {
                    match &mut current {
                        Some(item) => item.extra.append(&mut nested),
                        None => calendar.prelude.append(&mut nested),
                    }
                }
            }
            continue;
        }
        match upper.as_str() {
            "BEGIN:VEVENT" | "BEGIN:VTODO" => {
                let kind = if upper.ends_with("VEVENT") { Kind::Event } else { Kind::Todo };
                current = Some(Item::new(kind, "", ""));
                continue;
            }
            "END:VEVENT" | "END:VTODO" => {
                calendar.items.extend(current.take());
                continue;
            }
            "BEGIN:VCALENDAR" | "END:VCALENDAR" => continue,
            _ => {}
        }
        if upper.starts_with("BEGIN:") {
            depth = 1;
            nested.push(raw);
            continue;
        }
        let Some(line) = split(&raw) else { continue };
        match &mut current {
            Some(item) => read(item, line),
            None => {
                // Calendar-level properties such as PRODID and VERSION are written fresh.
                if !matches!(line.name.as_str(), "VERSION" | "PRODID" | "CALSCALE") {
                    calendar.prelude.push(line.raw);
                }
            }
        }
    }
    calendar
}

fn read(item: &mut Item, line: Line) {
    let when = || When::parse(&line.value, &line.params);
    match line.name.as_str() {
        "UID" => item.uid = line.value,
        "SUMMARY" => item.summary = unescape(&line.value),
        "DESCRIPTION" => item.description = unescape(&line.value),
        "LOCATION" => item.location = unescape(&line.value),
        "DTSTART" => item.start = when(),
        "DTEND" => item.end = when(),
        "DUE" => item.due = when(),
        "COMPLETED" => item.completed = when(),
        "STATUS" => item.status = line.value.to_ascii_uppercase(),
        "RRULE" => item.rrule = Some(line.value),
        "RECURRENCE-ID" => item.recurrence_id = when(),
        "EXDATE" => item.exdates.extend(line.value.split(',').filter_map(|part| When::parse(part, &line.params))),
        // A duration in place of an end.
        "DURATION" => {
            if let (Some(start), Some(length)) = (&item.start, duration(&line.value)) {
                item.end = Some(start.moved(start.naive() + length));
            }
        }
        _ => item.extra.push(line.raw),
    }
}

/// `P1DT2H30M`, `PT45M`, `P2W`.
fn duration(value: &str) -> Option<Duration> {
    let (negative, rest) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value.strip_prefix('+').unwrap_or(value)),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut number = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += match (unit, in_time) {
                    ('W', _) => Duration::weeks(n),
                    ('D', _) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(if negative { -total } else { total })
}

/// Writes `calendar` as a `VCALENDAR` with CRLF line ends, long lines folded at 75 bytes.
pub fn write(calendar: &Calendar) -> String {
    let mut lines: Vec<String> = vec!["BEGIN:VCALENDAR".into(), "VERSION:2.0".into(), "PRODID:-//Autark//autark-pim//EN".into()];
    lines.extend(calendar.prelude.iter().cloned());
    for item in &calendar.items {
        lines.push(format!("BEGIN:{}", item.kind.name()));
        lines.push(format!("UID:{}", item.uid));
        lines.push(format!("SUMMARY:{}", escape(&item.summary)));
        for (name, text) in [("DESCRIPTION", &item.description), ("LOCATION", &item.location)] {
            if !text.is_empty() {
                lines.push(format!("{name}:{}", escape(text)));
            }
        }
        for (name, when) in [("RECURRENCE-ID", &item.recurrence_id), ("DTSTART", &item.start), ("DTEND", &item.end), ("DUE", &item.due), ("COMPLETED", &item.completed)] {
            if let Some(when) = when {
                let (params, value) = when.write();
                lines.push(format!("{name}{params}:{value}"));
            }
        }
        if !item.status.is_empty() {
            lines.push(format!("STATUS:{}", item.status));
        }
        if let Some(rule) = &item.rrule {
            lines.push(format!("RRULE:{rule}"));
        }
        for exdate in &item.exdates {
            let (params, value) = exdate.write();
            lines.push(format!("EXDATE{params}:{value}"));
        }
        lines.extend(item.extra.iter().cloned());
        lines.push(format!("END:{}", item.kind.name()));
    }
    lines.push("END:VCALENDAR".into());
    let mut out = String::new();
    for line in lines {
        fold(&line, &mut out);
    }
    out
}

fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
}

/// One showing of an event: when it starts and ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub start: When,
    pub end: When,
}

/// The showings of `item` that overlap `[from, to)`, a repeating event's expanded by its rule. Supports
/// `FREQ` of DAILY, WEEKLY (with `BYDAY`), MONTHLY (the same day of the month) and YEARLY, with `INTERVAL`,
/// `COUNT` and `UNTIL`; a rule it can't read shows the event once. Edited occurrences (`recurrence_id`) are not
/// removed here: callers pass the series and the overrides to `with_overrides`.
pub fn occurrences(item: &Item, from: NaiveDateTime, to: NaiveDateTime) -> Vec<Occurrence> {
    let Some(start) = &item.start else { return Vec::new() };
    let length = item.length();
    let at = |moment: NaiveDateTime| Occurrence { start: start.moved(moment), end: start.moved(moment + length) };
    let overlaps = |moment: NaiveDateTime| moment < to && (moment + length > from || (length.is_zero() && moment >= from));
    let first = start.naive();
    let Some(rule) = item.rrule.as_deref().and_then(Rule::parse) else {
        return if overlaps(first) { vec![at(first)] } else { Vec::new() };
    };
    let skipped: Vec<NaiveDateTime> = item.exdates.iter().map(When::naive).collect();
    let mut found = Vec::new();
    let mut count = 0u32;
    for moment in rule.starts(first) {
        if rule.until.is_some_and(|until| moment > until) || rule.count.is_some_and(|max| count >= max) || moment >= to {
            break;
        }
        count += 1;
        // Date-only exclusions match on the day.
        let excluded = item.exdates.iter().any(|ex| if ex.is_date() { ex.naive().date() == moment.date() } else { ex.naive() == moment }) || skipped.contains(&moment);
        if !excluded && overlaps(moment) {
            found.push(at(moment));
        }
        // A rule with no end would run forever; stop long after the window.
        if count > 100_000 {
            break;
        }
    }
    found
}

struct Rule {
    freq: String,
    interval: u32,
    count: Option<u32>,
    until: Option<NaiveDateTime>,
    days: Vec<Weekday>,
}

impl Rule {
    fn parse(text: &str) -> Option<Rule> {
        let mut rule = Rule { freq: String::new(), interval: 1, count: None, until: None, days: Vec::new() };
        for part in text.split(';') {
            let (key, value) = part.split_once('=')?;
            match key.to_ascii_uppercase().as_str() {
                "FREQ" => rule.freq = value.to_ascii_uppercase(),
                "INTERVAL" => rule.interval = value.parse().ok().filter(|n| *n > 0)?,
                "COUNT" => rule.count = Some(value.parse().ok()?),
                "UNTIL" => rule.until = When::parse(value, &[]).map(|when| if when.is_date() { when.naive() + Duration::days(1) - Duration::seconds(1) } else { when.naive() }),
                "BYDAY" => {
                    for day in value.split(',') {
                        // Ordinals ("2MO") aren't supported.
                        let code = day.trim();
                        if code.len() != 2 {
                            return None;
                        }
                        rule.days.push(match code.to_ascii_uppercase().as_str() {
                            "MO" => Weekday::Mon,
                            "TU" => Weekday::Tue,
                            "WE" => Weekday::Wed,
                            "TH" => Weekday::Thu,
                            "FR" => Weekday::Fri,
                            "SA" => Weekday::Sat,
                            "SU" => Weekday::Sun,
                            _ => return None,
                        });
                    }
                }
                "WKST" => {}
                // BYMONTHDAY, BYSETPOS and the like: not supported, so the event shows once.
                "BYMONTHDAY" | "BYMONTH" | "BYSETPOS" | "BYYEARDAY" | "BYWEEKNO" | "BYHOUR" | "BYMINUTE" | "BYSECOND" => return None,
                _ => {}
            }
        }
        matches!(rule.freq.as_str(), "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY").then_some(rule)
    }

    /// Every start from `first` on, in order.
    fn starts(&self, first: NaiveDateTime) -> Box<dyn Iterator<Item = NaiveDateTime> + '_> {
        let time = first.time();
        let interval = i64::from(self.interval);
        match self.freq.as_str() {
            "DAILY" => Box::new((0i64..).map(move |n| first + Duration::days(n * interval))),
            "WEEKLY" => {
                let days: Vec<Weekday> = if self.days.is_empty() { vec![first.weekday()] } else { self.days.clone() };
                let monday = first.date() - Duration::days(i64::from(first.weekday().num_days_from_monday()));
                Box::new((0i64..).flat_map(move |week| {
                    let base = monday + Duration::weeks(week * interval);
                    let mut week_days: Vec<NaiveDateTime> = days.iter().map(|day| (base + Duration::days(i64::from(day.num_days_from_monday()))).and_time(time)).collect();
                    week_days.sort();
                    week_days.into_iter().filter(move |moment| *moment >= first)
                }))
            }
            "MONTHLY" => Box::new((0i64..).filter_map(move |n| add_months(first, n * interval))),
            _ => Box::new((0i64..).filter_map(move |n| add_months(first, n * interval * 12))),
        }
    }
}

/// `at` moved by `months`, or `None` when the day doesn't exist in that month (a 31st in April): such
/// months are skipped, as iCalendar says.
fn add_months(at: NaiveDateTime, months: i64) -> Option<NaiveDateTime> {
    let total = i64::from(at.year()) * 12 + i64::from(at.month0()) + months;
    let date = NaiveDate::from_ymd_opt(i32::try_from(total.div_euclid(12)).ok()?, u32::try_from(total.rem_euclid(12)).ok()? + 1, at.day())?;
    Some(date.and_time(at.time()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_time(NaiveTime::MIN)
    }

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//x//y//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/London\r\nBEGIN:STANDARD\r\nDTSTART:19701025T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:abc@example.com\r\nSUMMARY:Stand-up\\, daily\r\nDESCRIPTION:Line one\\nLine two that is quite long and so it folds somewhere around\r\n  here for sure\r\nDTSTART;TZID=Europe/London:20261019T090000\r\nDTEND;TZID=Europe/London:20261019T093000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4\r\nATTENDEE;CN=\"Sam; B\":mailto:sam@example.com\r\nBEGIN:VALARM\r\nTRIGGER:-PT10M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_an_event() {
        let calendar = parse(SAMPLE);
        assert_eq!(calendar.items.len(), 1);
        let item = &calendar.items[0];
        assert_eq!((item.uid.as_str(), item.summary.as_str()), ("abc@example.com", "Stand-up, daily"));
        assert!(item.description.starts_with("Line one\nLine two") && item.description.ends_with(" here for sure"));
        assert_eq!(item.start, Some(When::Zoned(day(2026, 10, 19) + Duration::hours(9), "Europe/London".into())));
        assert_eq!(item.length(), Duration::minutes(30));
        assert_eq!(item.extra.len(), 5, "{:?}", item.extra);
        assert_eq!(calendar.prelude.first().map(String::as_str), Some("BEGIN:VTIMEZONE"));
    }

    #[test]
    fn writing_keeps_what_it_does_not_read() {
        let again = parse(&write(&parse(SAMPLE)));
        assert_eq!(again, parse(SAMPLE));
        let text = write(&parse(SAMPLE));
        assert!(text.contains("BEGIN:VALARM") && text.contains("TZID:Europe/London") && text.contains("ATTENDEE;CN=\"Sam; B\":mailto:sam@example.com"));
        assert!(text.lines().all(|line| line.len() <= 75), "{text}");
    }

    #[test]
    fn weekly_rule_with_count() {
        let item = &parse(SAMPLE).items[0];
        let found = occurrences(item, day(2026, 10, 1), day(2027, 1, 1));
        let starts: Vec<NaiveDateTime> = found.iter().map(|o| o.start.naive()).collect();
        let nine = Duration::hours(9);
        assert_eq!(starts, vec![day(2026, 10, 19) + nine, day(2026, 10, 21) + nine, day(2026, 10, 26) + nine, day(2026, 10, 28) + nine]);
    }

    #[test]
    fn all_day_and_exclusions() {
        let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:1\nSUMMARY:Bins\nDTSTART;VALUE=DATE:20261020\nRRULE:FREQ=DAILY;INTERVAL=2\nEXDATE;VALUE=DATE:20261022\nEND:VEVENT\nEND:VCALENDAR\n";
        let item = &parse(text).items[0];
        assert_eq!(item.length(), Duration::days(1));
        let starts: Vec<NaiveDate> = occurrences(item, day(2026, 10, 20), day(2026, 10, 27)).iter().map(|o| o.start.naive().date()).collect();
        assert_eq!(starts, vec![NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 24).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()]);
    }

    #[test]
    fn monthly_skips_missing_days_and_unknown_rules_show_once() {
        let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:2\nSUMMARY:Rent\nDTSTART:20260131T100000Z\nRRULE:FREQ=MONTHLY\nEND:VEVENT\nBEGIN:VEVENT\nUID:3\nSUMMARY:Odd\nDTSTART:20261001T100000Z\nRRULE:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=FR\nEND:VEVENT\nEND:VCALENDAR\n";
        let calendar = parse(text);
        let months: Vec<u32> = occurrences(&calendar.items[0], day(2026, 1, 1), day(2026, 6, 1)).iter().map(|o| o.start.naive().month()).collect();
        assert_eq!(months, vec![1, 3, 5]);
        assert_eq!(occurrences(&calendar.items[1], day(2026, 1, 1), day(2027, 6, 1)).len(), 1);
    }

    #[test]
    fn todos_and_durations() {
        let text = "BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:t\nSUMMARY:Pay bill\nDUE;VALUE=DATE:20261101\nSTATUS:NEEDS-ACTION\nEND:VTODO\nBEGIN:VEVENT\nUID:e\nSUMMARY:Lunch\nDTSTART:20261019T120000\nDURATION:PT1H30M\nEND:VEVENT\nEND:VCALENDAR\n";
        let calendar = parse(text);
        assert_eq!(calendar.items[0].kind, Kind::Todo);
        assert!(!calendar.items[0].is_done());
        assert_eq!(calendar.items[1].length(), Duration::minutes(90));
        assert!(write(&calendar).contains("DUE;VALUE=DATE:20261101"));
    }
}
