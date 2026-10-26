//! When a scheduled job runs, read from words by code: "every day at 8", "weekdays at 17:30", "every monday at 9am",
//! "the 1st of every month at 9", "every morning". A 4B model gets times wrong and a job that runs on the wrong day is worse
//! than none, so the model never reads a time: it writes what to do and code owns when.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

/// The latest day of a month a job may be set for, so every month has it.
const LAST_DAY: u32 = 28;
/// How far back to look for the latest time a job was due: past any gap between runs.
const LOOKBACK: i64 = 62;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Days {
    Every,
    Weekdays,
    Weekends,
    On(Weekday),
    /// A day of the month, 1 to 28.
    OfMonth(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct When {
    pub days: Days,
    pub hour: u32,
    pub minute: u32,
}

const WEEKDAYS: [(&str, Weekday); 7] =
    [("monday", Weekday::Mon), ("tuesday", Weekday::Tue), ("wednesday", Weekday::Wed), ("thursday", Weekday::Thu), ("friday", Weekday::Fri), ("saturday", Weekday::Sat), ("sunday", Weekday::Sun)];

/// What can be said, for a message that explains a refusal.
pub const SHAPES: &str = "every day at 8, weekdays at 17:30, weekends at 10am, every monday at 9, the 1st of every month at 9, every morning (8), every evening (18) or every night (21)";

impl When {
    /// A job's time from words, or why not. The words need a time of day, or "morning", "evening" or "night".
    pub fn parse(text: &str) -> Result<Self, String> {
        let lower = text.to_lowercase();
        let words: Vec<String> = lower.split(|c: char| !(c.is_alphanumeric() || c == ':')).filter(|word| !word.is_empty()).map(str::to_owned).collect();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        if words.is_empty() {
            return Err(format!("say when, like {SHAPES}"));
        }
        let has = |word: &str| words.contains(&word);
        let weekday = WEEKDAYS.iter().find(|(name, _)| has(name) || has(&format!("{name}s"))).map(|(_, day)| *day);
        let of_month = words.windows(2).find_map(|pair| (pair[0] == "the" || pair[0] == "on").then(|| ordinal(pair[1])).flatten());
        let days = if has("weekdays") || has("weekday") {
            Days::Weekdays
        } else if has("weekends") || has("weekend") {
            Days::Weekends
        } else if let Some(day) = weekday {
            Days::On(day)
        } else if has("month") || has("monthly") {
            let day = of_month.ok_or_else(|| "say which day of the month, like \"the 1st of every month at 9\"".to_owned())?;
            if !(1..=LAST_DAY).contains(&day) {
                return Err(format!("a monthly job is for the 1st to the {LAST_DAY}th, so every month has the day"));
            }
            Days::OfMonth(day)
        } else if has("day") || has("daily") || has("morning") || has("evening") || has("night") || has("everyday") {
            Days::Every
        } else {
            return Err(format!("say when, like {SHAPES}"));
        };
        let usual = if has("morning") {
            Some((8, 0))
        } else if has("evening") {
            Some((18, 0))
        } else if has("night") {
            Some((21, 0))
        } else {
            None
        };
        let (hour, minute) = match clock(&words)? {
            Some(time) => time,
            None => usual.ok_or_else(|| format!("say what time, like {SHAPES}"))?,
        };
        Ok(Self { days, hour, minute })
    }

    /// The words for it, which `parse` reads back.
    pub fn describe(&self) -> String {
        let time = format!("{}:{:02}", self.hour, self.minute);
        match self.days {
            Days::Every => format!("every day at {time}"),
            Days::Weekdays => format!("weekdays at {time}"),
            Days::Weekends => format!("weekends at {time}"),
            Days::On(day) => format!("every {} at {time}", WEEKDAYS.iter().find(|(_, other)| *other == day).map_or("day", |(name, _)| name)),
            Days::OfMonth(day) => format!("the {} of every month at {time}", suffixed(day)),
        }
    }

    fn matches(&self, date: NaiveDate) -> bool {
        match self.days {
            Days::Every => true,
            Days::Weekdays => date.weekday().number_from_monday() <= 5,
            Days::Weekends => date.weekday().number_from_monday() > 5,
            Days::On(day) => date.weekday() == day,
            Days::OfMonth(day) => date.day() == day,
        }
    }

    fn time(&self) -> NaiveTime {
        NaiveTime::from_hms_opt(self.hour, self.minute, 0).unwrap_or_default()
    }

    /// The latest time at or before `now` it was due.
    pub fn latest_due(&self, now: NaiveDateTime) -> Option<NaiveDateTime> {
        (0..LOOKBACK).map(|back| now.date() - Duration::days(back)).filter(|date| self.matches(*date)).map(|date| date.and_time(self.time())).find(|due| *due <= now)
    }

    /// The next time it is due, after `now`.
    pub fn next_due(&self, now: NaiveDateTime) -> Option<NaiveDateTime> {
        (0..LOOKBACK).map(|ahead| now.date() + Duration::days(ahead)).filter(|date| self.matches(*date)).map(|date| date.and_time(self.time())).find(|due| *due > now)
    }
}

/// "1st", "2nd", "15th", or a bare "15", as a day of the month.
fn ordinal(word: &str) -> Option<u32> {
    let digits = word.trim_end_matches(|c: char| c.is_alphabetic());
    let suffix = &word[digits.len()..];
    (matches!(suffix, "" | "st" | "nd" | "rd" | "th") && !digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn suffixed(day: u32) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

/// A time of day in the words: "8am", "9:30 pm", "17:30", or a bare number after "at". `Ok(None)` when there is none,
/// `Err` when it gives one that can't be.
fn clock(words: &[&str]) -> Result<Option<(u32, u32)>, String> {
    let bad = || "that time of day isn't one".to_owned();
    for (n, word) in words.iter().enumerate() {
        let (number, suffix) = match word.find(|c: char| c.is_alphabetic()) {
            Some(at) if word[at..] == *"am" || word[at..] == *"pm" => (&word[..at], Some(&word[at..])),
            Some(_) => continue,
            None => (*word, words.get(n + 1).copied().filter(|next| matches!(*next, "am" | "pm"))),
        };
        let (hour, minute) = match number.split_once(':') {
            Some((h, m)) => (h.parse::<u32>().map_err(|_| bad())?, m.parse::<u32>().map_err(|_| bad())?),
            None if suffix.is_some() || (n > 0 && words[n - 1] == "at") => (number.parse::<u32>().map_err(|_| bad())?, 0),
            None => continue,
        };
        if minute > 59 {
            return Err(bad());
        }
        let hour = match suffix {
            Some("pm") if (1..=12).contains(&hour) => hour % 12 + 12,
            Some("am") if (1..=12).contains(&hour) => hour % 12,
            Some(_) => return Err(bad()),
            None => hour,
        };
        return if hour < 24 { Ok(Some((hour, minute))) } else { Err(bad()) };
    }
    Ok(None)
}

/// What starts a schedule in a sentence.
const STARTS: [&str; 6] = ["every", "each", "daily", "weekdays", "weekends", "monthly"];
/// What a sentence about yourself starts with, which is not an instruction to run: "I go to the gym every day at 6".
const NOT_INSTRUCTIONS: [&str; 10] = ["i", "i'm", "i’m", "we", "he", "she", "they", "it", "my", "there"];

fn key(token: &str) -> String {
    token.trim_matches(|c: char| !c.is_alphanumeric() && c != ':' && c != '\'' && c != '’').to_lowercase()
}

fn is_clock(key: &str) -> bool {
    let digits = key.trim_end_matches("am").trim_end_matches("pm");
    digits.chars().next().is_some_and(|c| c.is_ascii_digit()) && digits.chars().all(|c| c.is_ascii_digit() || c == ':')
}

/// What to do and when, from a sentence that asks for something on a schedule: "every weekday at 8, summarise my
/// calendar" or "summarise my calendar every weekday at 8am". `None` for anything else, a question or a sentence about
/// yourself included.
pub fn request(said: &str) -> Option<(When, String)> {
    let said = said.trim();
    if said.ends_with('?') {
        return None;
    }
    let tokens: Vec<&str> = said.split_whitespace().collect();
    let keys: Vec<String> = tokens.iter().map(|token| key(token)).collect();
    let on_days = |n: usize| keys[n] == "on" && keys.get(n + 1).is_some_and(|next| next == "weekdays" || next == "weekends" || WEEKDAYS.iter().any(|(name, _)| next == &format!("{name}s")));
    let start = (0..tokens.len()).find(|n| STARTS.contains(&keys[*n].as_str()) || on_days(*n))?;
    let (phrase, what): (Vec<&str>, Vec<&str>) = if start == 0 {
        // The phrase ends at the time of day, or at "morning", "evening" or "night".
        let end = (start..tokens.len()).find(|n| {
            (is_clock(&keys[*n]) && (keys[*n].ends_with("am") || keys[*n].ends_with("pm") || keys[*n].contains(':') || (*n > 0 && keys[*n - 1] == "at")))
                || ["morning", "evening", "night"].contains(&keys[*n].as_str())
        })?;
        let end = if keys.get(end + 1).is_some_and(|next| next == "am" || next == "pm") { end + 1 } else { end };
        (tokens[start..=end].to_vec(), tokens[end + 1..].to_vec())
    } else {
        (tokens[start..].to_vec(), tokens[..start].to_vec())
    };
    let when = When::parse(&phrase.join(" ")).ok()?;
    let mut what = what.join(" ");
    // "..., please", "then", "to": the words around the instruction.
    loop {
        let trimmed = what.trim_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ':' | ';' | '-' | '.')).to_owned();
        let lower = trimmed.to_lowercase();
        let stripped = ["please ", "then ", "and ", "to "].iter().find_map(|lead| lower.starts_with(lead).then(|| trimmed[lead.len()..].to_owned())).unwrap_or(trimmed.clone());
        if stripped == what {
            break;
        }
        what = stripped;
    }
    let first = what.split_whitespace().next().map(key)?;
    (what.chars().count() >= 4 && !NOT_INSTRUCTIONS.contains(&first.as_str())).then_some((when, what))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        // October 2026: the 19th is a Monday.
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap().and_hms_opt(hour, minute, 0).unwrap()
    }

    fn when(days: Days, hour: u32, minute: u32) -> When {
        When { days, hour, minute }
    }

    #[test]
    fn the_shapes_are_read() {
        assert_eq!(When::parse("every day at 8"), Ok(when(Days::Every, 8, 0)));
        assert_eq!(When::parse("Daily at 8:30am"), Ok(when(Days::Every, 8, 30)));
        assert_eq!(When::parse("weekdays at 5:30 pm"), Ok(when(Days::Weekdays, 17, 30)));
        assert_eq!(When::parse("every weekday at 17:30"), Ok(when(Days::Weekdays, 17, 30)));
        assert_eq!(When::parse("on weekends at 10am"), Ok(when(Days::Weekends, 10, 0)));
        assert_eq!(When::parse("every Monday at 9"), Ok(when(Days::On(Weekday::Mon), 9, 0)));
        assert_eq!(When::parse("on fridays at 4pm"), Ok(when(Days::On(Weekday::Fri), 16, 0)));
        assert_eq!(When::parse("the 1st of every month at 9"), Ok(when(Days::OfMonth(1), 9, 0)));
        assert_eq!(When::parse("every month on the 15th at 7:15"), Ok(when(Days::OfMonth(15), 7, 15)));
        assert_eq!(When::parse("every morning"), Ok(when(Days::Every, 8, 0)));
        assert_eq!(When::parse("every evening"), Ok(when(Days::Every, 18, 0)));
        assert_eq!(When::parse("every night at 10pm"), Ok(when(Days::Every, 22, 0)));
        assert_eq!(When::parse("12am every day"), Ok(when(Days::Every, 0, 0)));
    }

    #[test]
    fn what_is_not_a_time_says_what_can_be_said() {
        for text in ["", "sometimes", "every day", "weekdays", "every day at 25", "every day at 8:75", "every month at 9", "the 31st of every month at 9", "every day at 13pm"] {
            assert!(When::parse(text).is_err(), "{text}");
        }
        assert!(When::parse("every day").unwrap_err().contains("every day at 8"));
    }

    #[test]
    fn what_is_described_is_read_back_the_same() {
        for text in [
            "every day at 8:00",
            "weekdays at 17:30",
            "weekends at 10:05",
            "every monday at 9:00",
            "the 1st of every month at 9:00",
            "the 22nd of every month at 7:15",
            "the 13th of every month at 0:00",
        ] {
            let parsed = When::parse(text).unwrap();
            assert_eq!(parsed.describe(), text);
            assert_eq!(When::parse(&parsed.describe()), Ok(parsed));
        }
    }

    #[test]
    fn the_latest_due_time_is_the_last_one_at_or_before_now() {
        let weekdays = when(Days::Weekdays, 8, 0);
        // Monday 9:00: today's 8:00. Monday 7:00: Friday's. Sunday: Friday's.
        assert_eq!(weekdays.latest_due(at(19, 9, 0)), Some(at(19, 8, 0)));
        assert_eq!(weekdays.latest_due(at(19, 7, 0)), Some(at(16, 8, 0)));
        assert_eq!(weekdays.latest_due(at(18, 12, 0)), Some(at(16, 8, 0)));
        assert_eq!(weekdays.latest_due(at(19, 8, 0)), Some(at(19, 8, 0)));
        assert_eq!(when(Days::OfMonth(1), 9, 0).latest_due(at(19, 12, 0)), Some(at(1, 9, 0)));
        assert_eq!(when(Days::On(Weekday::Mon), 9, 0).latest_due(at(25, 12, 0)), Some(at(19, 9, 0)));
    }

    #[test]
    fn the_next_due_time_is_the_first_after_now() {
        assert_eq!(when(Days::Every, 8, 0).next_due(at(19, 9, 0)), Some(at(20, 8, 0)));
        assert_eq!(when(Days::Every, 8, 0).next_due(at(19, 7, 0)), Some(at(19, 8, 0)));
        assert_eq!(when(Days::Weekdays, 8, 0).next_due(at(23, 9, 0)), Some(at(26, 8, 0)));
        assert_eq!(when(Days::OfMonth(1), 9, 0).next_due(at(19, 9, 0)).map(|due| due.date()), NaiveDate::from_ymd_opt(2026, 11, 1));
    }

    #[test]
    fn a_sentence_that_asks_for_something_on_a_schedule_gives_what_and_when() {
        let (w, what) = request("Every weekday at 8, summarise my calendar").unwrap();
        assert_eq!((w, what.as_str()), (when(Days::Weekdays, 8, 0), "summarise my calendar"));
        let (w, what) = request("every morning please tell me who has written").unwrap();
        assert_eq!((w, what.as_str()), (when(Days::Every, 8, 0), "tell me who has written"));
        let (w, what) = request("Summarise my calendar every weekday at 8am").unwrap();
        assert_eq!((w, what.as_str()), (when(Days::Weekdays, 8, 0), "Summarise my calendar"));
        let (w, what) = request("every monday at 9:30 am, to list what is due this week.").unwrap();
        assert_eq!((w, what.as_str()), (when(Days::On(Weekday::Mon), 9, 30), "list what is due this week"));
        let (w, what) = request("on weekends at 10 check the forecast").unwrap();
        assert_eq!((w, what.as_str()), (when(Days::Weekends, 10, 0), "check the forecast"));
        let (w, _) = request("every month on the 1st at 9, show me my budget notes").unwrap();
        assert_eq!(w, when(Days::OfMonth(1), 9, 0));
    }

    #[test]
    fn questions_and_sentences_about_yourself_are_not_schedules() {
        for text in [
            "what do I do every day at 8?",
            "I go to the gym every day at 6",
            "my meeting is every monday at 9",
            "every day at 8",
            "every day I walk the dog",
            "tell me a joke",
            "every day at 8, hi",
            "we meet on weekdays at 9 am",
        ] {
            assert_eq!(request(text), None, "{text}");
        }
    }
}
