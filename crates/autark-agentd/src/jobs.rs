//! An agent's scheduled jobs: `jobs.toml` in its folder, a `[[job]]` for each, with what to do and when. A job runs as if you
//! had said its prompt to the agent, in the agent's conversation, so the answer is waiting when you open the roster. Each
//! keeps the time it last ran for, so one missed while the laptop slept is made up once on waking, and not more than once.

use std::path::Path;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::when::When;

/// How many jobs an agent has, and how long what it is to do may be.
pub const MAX_JOBS: usize = 5;
pub const PROMPT: usize = 400;
/// How a time is kept in the file.
const FORMAT: &str = "%Y-%m-%dT%H:%M";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Job {
    /// The words for when, as `When::describe` writes them.
    pub when: String,
    pub prompt: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The time it was last due and run, or made up for, as local time; empty before it has been.
    #[serde(default)]
    pub last: String,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    job: Vec<Job>,
}

pub fn load(path: &Path) -> Vec<Job> {
    std::fs::read_to_string(path).ok().and_then(|text| toml::from_str::<File>(&text).ok()).map(|file| file.job).unwrap_or_default()
}

fn save(path: &Path, jobs: Vec<Job>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    }
    let text = if jobs.is_empty() { String::new() } else { toml::to_string(&File { job: jobs }).map_err(|err| err.to_string())? };
    std::fs::write(path, text).map_err(|err| format!("writing {}: {err}", path.display()))
}

/// A job that starts from now: what was due before it was made doesn't run.
fn starting(when: &When, now: NaiveDateTime) -> String {
    when.latest_due(now).map(|due| due.format(FORMAT).to_string()).unwrap_or_default()
}

/// Adds a job, or says why it can't be.
pub fn add(path: &Path, when: &When, prompt: &str, now: NaiveDateTime) -> Result<(), String> {
    let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if prompt.chars().count() < 2 {
        return Err("say what it is to do".to_owned());
    }
    if prompt.chars().count() > PROMPT {
        return Err(format!("what it is to do is at most {PROMPT} characters"));
    }
    let mut jobs = load(path);
    if jobs.len() >= MAX_JOBS {
        return Err(format!("an agent has at most {MAX_JOBS} jobs: remove one first"));
    }
    let when_text = when.describe();
    if jobs.iter().any(|job| job.when == when_text && job.prompt.eq_ignore_ascii_case(&prompt)) {
        return Ok(());
    }
    jobs.push(Job { when: when_text, prompt, enabled: true, last: starting(when, now) });
    save(path, jobs)
}

pub fn remove(path: &Path, n: usize) -> Result<(), String> {
    let mut jobs = load(path);
    if n >= jobs.len() {
        return Err("there is no such job".to_owned());
    }
    jobs.remove(n);
    save(path, jobs)
}

/// Turns a job on or off. One turned on starts from now, so what was due while it was off doesn't run.
pub fn set_enabled(path: &Path, n: usize, on: bool, now: NaiveDateTime) -> Result<(), String> {
    let mut jobs = load(path);
    let job = jobs.get_mut(n).ok_or_else(|| "there is no such job".to_owned())?;
    if on && !job.enabled {
        job.last = When::parse(&job.when).ok().map(|when| starting(&when, now)).unwrap_or_default();
    }
    job.enabled = on;
    save(path, jobs)
}

/// The jobs that are due at `now`, each with the time it was due: on, and due after it last ran.
pub fn due(path: &Path, now: NaiveDateTime) -> Vec<(usize, NaiveDateTime)> {
    load(path)
        .iter()
        .enumerate()
        .filter(|(_, job)| job.enabled)
        .filter_map(|(n, job)| {
            let latest = When::parse(&job.when).ok()?.latest_due(now)?;
            let last = NaiveDateTime::parse_from_str(&job.last, FORMAT).ok();
            last.is_none_or(|last| latest > last).then_some((n, latest))
        })
        .collect()
}

/// Notes that job `n` ran for the time it was due.
pub fn ran(path: &Path, n: usize, due: NaiveDateTime) {
    let mut jobs = load(path);
    if let Some(job) = jobs.get_mut(n) {
        job.last = due.format(FORMAT).to_string();
        if let Err(err) = save(path, jobs) {
            eprintln!("agentd: noting a job's run: {err}");
        }
    }
}

/// The jobs as JSON for a client: `[{"n","when","prompt","enabled","next","last"}]`, `next` as local time.
pub fn view(path: &Path, now: NaiveDateTime) -> Value {
    Value::Array(
        load(path)
            .iter()
            .enumerate()
            .map(|(n, job)| {
                let next = job.enabled.then(|| When::parse(&job.when).ok().and_then(|when| when.next_due(now))).flatten().map(|next| next.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                json!({ "n": n, "when": job.when, "prompt": job.prompt, "enabled": job.enabled, "next": next, "last": job.last })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn path(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agentd-jobs-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("jobs.toml")
    }

    /// October 2026: the 19th is a Monday.
    fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap().and_hms_opt(hour, minute, 0).unwrap()
    }

    fn weekdays(hour: u32) -> When {
        When::parse(&format!("weekdays at {hour}")).unwrap()
    }

    #[test]
    fn a_job_is_kept_with_its_words_and_starts_from_now() {
        let p = path("add");
        add(&p, &weekdays(8), "  Summarise   my calendar ", at(19, 9, 0)).unwrap();
        add(&p, &weekdays(8), "summarise my calendar", at(19, 9, 0)).unwrap();
        let jobs = load(&p);
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].when.as_str(), jobs[0].prompt.as_str(), jobs[0].enabled), ("weekdays at 8:00", "Summarise my calendar", true));
        // What was due before it was made doesn't run.
        assert!(due(&p, at(19, 9, 0)).is_empty());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_job_is_due_once_each_time_and_a_missed_one_is_made_up_once() {
        let p = path("due");
        add(&p, &weekdays(8), "Summarise", at(19, 9, 0)).unwrap();
        assert!(due(&p, at(19, 23, 0)).is_empty());
        // Tuesday 8:00 comes.
        assert_eq!(due(&p, at(20, 8, 0)), [(0, at(20, 8, 0))]);
        ran(&p, 0, at(20, 8, 0));
        assert!(due(&p, at(20, 8, 30)).is_empty());
        // The laptop sleeps through Wednesday and Thursday: on waking Friday at 10, it runs once, for Friday.
        assert_eq!(due(&p, at(23, 10, 0)), [(0, at(23, 8, 0))]);
        ran(&p, 0, at(23, 8, 0));
        assert!(due(&p, at(23, 10, 1)).is_empty());
        // Saturday and Sunday it is not due; Monday it is.
        assert!(due(&p, at(25, 12, 0)).is_empty());
        assert_eq!(due(&p, at(26, 8, 0)).len(), 1);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_job_that_is_off_doesnt_run_and_one_turned_on_starts_from_now() {
        let p = path("off");
        add(&p, &weekdays(8), "Summarise", at(19, 9, 0)).unwrap();
        set_enabled(&p, 0, false, at(19, 9, 0)).unwrap();
        assert!(due(&p, at(21, 9, 0)).is_empty());
        set_enabled(&p, 0, true, at(21, 9, 0)).unwrap();
        assert!(due(&p, at(21, 9, 0)).is_empty());
        assert_eq!(due(&p, at(22, 8, 0)).len(), 1);
        assert!(set_enabled(&p, 3, true, at(21, 9, 0)).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn jobs_are_limited_and_can_be_removed() {
        let p = path("limits");
        for n in 0..MAX_JOBS {
            add(&p, &weekdays(8), &format!("job {n}"), at(19, 9, 0)).unwrap();
        }
        assert!(add(&p, &weekdays(8), "one more", at(19, 9, 0)).unwrap_err().contains("at most"));
        assert!(add(&p, &weekdays(8), &"x".repeat(PROMPT + 1), at(19, 9, 0)).is_err());
        assert!(add(&p, &weekdays(8), " ", at(19, 9, 0)).is_err());
        remove(&p, 0).unwrap();
        assert_eq!(load(&p).len(), MAX_JOBS - 1);
        assert!(remove(&p, 9).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn the_view_has_what_a_client_shows() {
        let p = path("view");
        add(&p, &weekdays(8), "Summarise", at(19, 9, 0)).unwrap();
        let view = view(&p, at(19, 9, 0));
        assert_eq!((view[0]["n"].as_u64(), view[0]["when"].as_str(), view[0]["next"].as_str()), (Some(0), Some("weekdays at 8:00"), Some("2026-10-20 08:00")));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
