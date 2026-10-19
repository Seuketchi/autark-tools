//! One-off times, read from the sentence by code: "in 30 minutes", "in an hour", "tomorrow at 9am", "tonight at 8",
//! "today at 17:30", "next monday at 10". A 4B model gets dates wrong, and a reminder that fires on the wrong day is
//! worse than none, so the model writes the task's name and message and code owns when it runs.
//!
//! Only a request that says when, by one of those shapes, is a one-off: it has to ask to be reminded or told,
//! so "how many hours are in a day" is a question and not a schedule. "Remind me to sleep by 10pm", with no
//! day, stays a daily reminder, and anything with "every", "each" or "daily" in it is recurring, never one-off.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};

/// The latest a one-off may be, so a slip like "in 900 hours" is not scheduled.
const MAX_DAYS: i64 = 60;

/// When a one-off sentence says it should happen, to the minute, after `now`; `None` if it says no such time.
pub fn once(text: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let text = text.to_lowercase();
    if ["every", "each ", "daily", "weekly", "monthly", "nightly", "hourly", "weekday", "weekend"].iter().any(|word| text.contains(word)) {
        return None;
    }
    if !["remind", "ping me", "alert me", "notify me", "nudge", "tell me", "let me know", "wake me", "check ", "run ", "send me", "give me", "show me"].iter().any(|cue| text.contains(cue)) {
        return None;
    }
    let words: Vec<String> = text.split(|c: char| !(c.is_alphanumeric() || c == ':')).filter(|word| !word.is_empty()).map(str::to_owned).collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let now = now.with_second_zero();
    let found = relative(&words, now).or_else(|| dated(&words, now))?;
    (found > now && found <= now + Duration::days(MAX_DAYS)).then_some(found)
}

trait Minutes {
    fn with_second_zero(self) -> Self;
}

impl Minutes for NaiveDateTime {
    fn with_second_zero(self) -> Self {
        self.date().and_time(NaiveTime::from_hms_opt(chrono::Timelike::hour(&self), chrono::Timelike::minute(&self), 0).unwrap_or_default())
    }
}

/// "in 30 minutes", "in an hour", "in half an hour", "in 2 hours".
fn relative(words: &[&str], now: NaiveDateTime) -> Option<NaiveDateTime> {
    let at = words.iter().position(|word| *word == "in")?;
    let rest = &words[at + 1..];
    let (amount, unit) = match rest {
        ["half", "an" | "a", "hour" | "hr", ..] => (30.0, "minute"),
        ["an" | "a", unit, ..] => (1.0, *unit),
        [number, unit, ..] => (number.parse::<f64>().ok()?, *unit),
        _ => return None,
    };
    let minutes = match unit.trim_end_matches('s') {
        "minute" | "min" => amount,
        "hour" | "hr" => amount * 60.0,
        "day" => amount * 1440.0,
        _ => return None,
    };
    (minutes >= 1.0).then(|| now + Duration::minutes(minutes as i64))
}

/// "tomorrow", "tonight", "today", "next monday" or "on monday", each with a time or its usual one.
fn dated(words: &[&str], now: NaiveDateTime) -> Option<NaiveDateTime> {
    let today = now.date();
    let (date, default, evening): (NaiveDate, Option<(u32, u32)>, bool) = if words.contains(&"tomorrow") {
        (today.succ_opt()?, Some((9, 0)), false)
    } else if words.contains(&"tonight") {
        (today, Some((20, 0)), true)
    } else if words.contains(&"today") {
        (today, None, false)
    } else if let Some(day) = weekday_after(words) {
        let ahead = (day.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
        (today + Duration::days(if ahead == 0 { 7 } else { ahead }), Some((9, 0)), false)
    } else {
        return None;
    };
    // A time the sentence gives but can't be (25:00) is no time at all, not the usual one.
    let (hour, minute) = clock(words, evening).ok()?.or(default)?;
    Some(date.and_time(NaiveTime::from_hms_opt(hour, minute, 0)?))
}

/// The weekday after "next" or "on".
fn weekday_after(words: &[&str]) -> Option<chrono::Weekday> {
    words.windows(2).find_map(|pair| {
        if !matches!(pair[0], "next" | "on") {
            return None;
        }
        Some(match pair[1] {
            "monday" => chrono::Weekday::Mon,
            "tuesday" => chrono::Weekday::Tue,
            "wednesday" => chrono::Weekday::Wed,
            "thursday" => chrono::Weekday::Thu,
            "friday" => chrono::Weekday::Fri,
            "saturday" => chrono::Weekday::Sat,
            "sunday" => chrono::Weekday::Sun,
            _ => return None,
        })
    })
}

/// A time of day in the words: "9am", "9:30 pm", "17:30", or a bare number after "at" or "by". With `evening`, a bare
/// 1 to 11 is the evening ("tonight at 8"). `Ok(None)` when there is none, `Err` when it gives one that can't be.
fn clock(words: &[&str], evening: bool) -> Result<Option<(u32, u32)>, ()> {
    for (n, word) in words.iter().enumerate() {
        let (number, suffix) = match word.find(|c: char| c.is_alphabetic()) {
            Some(at) if word[at..] == *"am" || word[at..] == *"pm" => (&word[..at], Some(&word[at..])),
            Some(_) => continue,
            None => (*word, words.get(n + 1).copied().filter(|next| matches!(*next, "am" | "pm"))),
        };
        let (hour, minute) = match number.split_once(':') {
            Some((h, m)) => (h.parse::<u32>().map_err(drop)?, m.parse::<u32>().map_err(drop)?),
            None if suffix.is_some() || (n > 0 && matches!(words[n - 1], "at" | "by" | "around")) => (number.parse::<u32>().map_err(drop)?, 0),
            None => continue,
        };
        if minute > 59 {
            return Err(());
        }
        let hour = match suffix {
            Some("pm") if (1..=12).contains(&hour) => hour % 12 + 12,
            Some("am") if (1..=12).contains(&hour) => hour % 12,
            Some(_) => return Err(()),
            None if evening && (1..=11).contains(&hour) => hour + 12,
            None => hour,
        };
        return if hour < 24 { Ok(Some((hour, minute))) } else { Err(()) };
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monday 19 October 2026, 18:00.
    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 19).unwrap().and_hms_opt(18, 0, 17).unwrap()
    }

    fn at(day: u32, hour: u32, minute: u32) -> Option<NaiveDateTime> {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap().and_hms_opt(hour, minute, 0)
    }

    #[test]
    fn it_reads_how_long_from_now() {
        assert_eq!(once("remind me to stretch in 30 minutes", now()), at(19, 18, 30));
        assert_eq!(once("remind me in an hour to call", now()), at(19, 19, 0));
        assert_eq!(once("ping me in half an hour", now()), at(19, 18, 30));
        assert_eq!(once("ping me in half a minute", now()), None, "only half an hour is a thing");
        assert_eq!(once("in 2 hours check the build", now()), at(19, 20, 0));
        assert_eq!(once("remind me in 3 days to renew it", now()), at(22, 18, 0));
        assert_eq!(once("remind me in 0 minutes", now()), None);
        assert_eq!(once("remind me in 2000 hours", now()), None, "too far ahead to be a slip of the tongue");
    }

    #[test]
    fn it_reads_a_day_and_a_time() {
        assert_eq!(once("remind me tomorrow at 9am to send it", now()), at(20, 9, 0));
        assert_eq!(once("tomorrow at 7:45 pm remind me to call mum", now()), at(20, 19, 45));
        assert_eq!(once("remind me tomorrow to pay rent", now()), at(20, 9, 0), "a day alone is 9 in the morning");
        assert_eq!(once("tonight at 8 remind me to stretch", now()), at(19, 20, 0));
        assert_eq!(once("remind me tonight to sleep", now()), at(19, 20, 0));
        assert_eq!(once("today at 17:30 remind me", now()), None, "already past");
        assert_eq!(once("today at 9pm remind me", now()), at(19, 21, 0));
        assert_eq!(once("remind me next monday at 10 to review", now()), at(26, 10, 0), "not today, which is Monday");
        assert_eq!(once("remind me on friday at 3pm to book it", now()), at(23, 15, 0));
        assert_eq!(once("remind me tomorrow at 12am", now()), at(20, 0, 0));
        assert_eq!(once("remind me tomorrow at 12pm", now()), at(20, 12, 0));
    }

    #[test]
    fn a_sentence_without_such_a_time_is_not_one_off() {
        assert_eq!(once("remind me to sleep by 10pm", now()), None, "no day: that stays a daily reminder");
        assert_eq!(once("every morning at 8 tell me the news", now()), None);
        assert_eq!(once("remind me every day tomorrow at 9", now()), None, "recurring wins");
        assert_eq!(once("what's the news today", now()), None, "today needs a time");
        assert_eq!(once("how many hours are in a day", now()), None);
        assert_eq!(once("remind me tomorrow at 25:00", now()), None);
        assert_eq!(once("remind me tomorrow at 9:75", now()), None);
        assert_eq!(once("tomorrow at 9am it rains", now()), None, "a statement is not a request");
    }
}
