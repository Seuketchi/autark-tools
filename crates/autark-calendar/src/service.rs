//! `org.autark.Calendar1`: the calendar for Kith (and scripts) while the app is running. Reading is open to the
//! session; changing asks nothing here because Kith asks you first, on a card, before it calls.

use std::sync::Arc;

use chrono::{Duration, NaiveDate};
use serde_json::json;

use crate::backend::Backend;
use crate::logic;

pub const PATH: &str = "/org/autark/Calendar1";

pub struct Service {
    pub backend: Arc<Backend>,
}

fn failed(why: String) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(why)
}

fn day(text: &str) -> zbus::fdo::Result<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").map_err(|_| failed(format!("“{text}” isn't a day like 2026-10-19")))
}

#[zbus::interface(name = "org.autark.Calendar1")]
impl Service {
    /// The calendars, as JSON: `[{"account","name","url","color"}]`.
    async fn calendars(&self) -> String {
        let list: Vec<_> = self.backend.snapshot().iter().map(|cal| json!({ "account": cal.account_name, "name": cal.name, "url": cal.url, "color": cal.color })).collect();
        json!(list).to_string()
    }

    /// The events on `days` days from `from` (`YYYY-MM-DD`), as JSON, soonest first, fetched fresh.
    async fn agenda(&self, from: String, days: u32) -> zbus::fdo::Result<String> {
        let (first, days) = (day(&from)?, i64::from(days.clamp(1, 62)));
        let backend = self.backend.clone();
        let problems = blocking::unblock(move || backend.refresh(first - Duration::days(1), first + Duration::days(days + 1))).await;
        if let Some(why) = problems.first().filter(|_| self.backend.snapshot().is_empty()) {
            return Err(failed(why.clone()));
        }
        let mut events = Vec::new();
        for offset in 0..days {
            let date = first + Duration::days(offset);
            for cal in self.backend.snapshot() {
                for stored in &cal.resources {
                    for showing in logic::showings(&stored.parsed, &stored.url, &cal.name, &cal.color, date) {
                        events.push(json!({
                            "title": showing.summary, "start": showing.start.format("%Y-%m-%dT%H:%M").to_string(), "end": showing.end.format("%Y-%m-%dT%H:%M").to_string(),
                            "all_day": showing.all_day, "place": showing.location, "calendar": showing.calendar, "event": showing.uid, "resource": showing.resource,
                        }));
                    }
                }
            }
        }
        Ok(json!(events).to_string())
    }

    /// Adds an event to the calendar at `calendar` (its `url` from `Calendars`). With empty `start` and `end`
    /// it is all day. Returns the event's ID.
    async fn create_event(&self, calendar: String, title: String, date: String, start: String, end: String, place: String, notes: String) -> zbus::fdo::Result<String> {
        let date = day(&date)?;
        let times = if start.trim().is_empty() {
            None
        } else {
            Some((logic::parse_time(&start).ok_or_else(|| failed(format!("“{start}” isn't a time like 09:30")))?, logic::parse_time(&end).ok_or_else(|| failed(format!("“{end}” isn't a time like 10:30")))?))
        };
        let uid = crate::editor::new_uid();
        let mut event = logic::new_event(&uid, title.trim(), date, times, 1);
        event.location = place;
        event.description = notes;
        let cal = self.backend.snapshot().into_iter().find(|cal| cal.url == calendar).ok_or_else(|| failed("no such calendar".to_owned()))?;
        let backend = self.backend.clone();
        blocking::unblock(move || backend.create(&cal.account, &cal.url, event)).await.map_err(failed)?;
        Ok(uid)
    }

    /// Deletes the event whose `resource` an agenda entry gave.
    async fn delete_event(&self, resource: String) -> zbus::fdo::Result<()> {
        let backend = self.backend.clone();
        blocking::unblock(move || backend.delete(&resource)).await.map_err(failed)
    }
}
