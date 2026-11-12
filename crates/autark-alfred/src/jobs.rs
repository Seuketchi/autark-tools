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
/// The version of `jobs.toml` this writes.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Job {
    /// What the job is called by: never given to another of the agent's jobs, so a client or a run under way that holds
    /// it never reaches another job. 0 in a file written before jobs had ids, until it is read.
    #[serde(default)]
    pub id: u32,
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
    /// Missing in the first files, which are version 1.
    #[serde(default = "first")]
    version: u32,
    /// The id the next job gets.
    #[serde(default)]
    next: u32,
    #[serde(default)]
    job: Vec<Job>,
}

fn first() -> u32 {
    1
}

/// The file at `path`, with an id for each job; empty when it hasn't been made. A file that can't be understood is moved
/// aside to `jobs.toml.bad-<seconds>`, so a new one never writes over it; one from a newer version is an error and left
/// as it is.
fn read(path: &Path) -> Result<File, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(File { next: 1, ..File::default() }),
        Err(err) => return Err(format!("its jobs.toml can't be read: {err}")),
    };
    let mut file = match toml::from_str::<File>(&text) {
        Ok(file) if file.version > VERSION => return Err(format!("its jobs.toml is from a newer version (format {}), so it is left as it is", file.version)),
        Ok(file) => file,
        Err(err) => {
            let aside = crate::files::set_aside(path, crate::chat::now()).map_err(|moving| format!("its jobs.toml can't be understood ({err}) or moved aside: {moving}"))?;
            eprintln!("alfred: {} couldn't be understood, so it was moved to {}: {err}", path.display(), aside.display());
            File::default()
        }
    };
    // Jobs written before they had ids get them in order, the same each time until the file is written again.
    file.next = file.next.max(file.job.iter().map(|job| job.id).max().unwrap_or(0) + 1);
    for job in file.job.iter_mut().filter(|job| job.id == 0) {
        job.id = file.next;
        file.next += 1;
    }
    Ok(file)
}

/// The jobs kept at `path`, as `read` finds them.
pub fn load(path: &Path) -> Result<Vec<Job>, String> {
    read(path).map(|file| file.job)
}

fn save(path: &Path, mut file: File) -> Result<(), String> {
    file.version = VERSION;
    let text = toml::to_string(&file).map_err(|err| err.to_string())?;
    crate::files::write(path, text.as_bytes()).map_err(|err| format!("writing {}: {err}", path.display()))
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
    let mut file = read(path)?;
    if file.job.len() >= MAX_JOBS {
        return Err(format!("an agent has at most {MAX_JOBS} jobs: remove one first"));
    }
    let when_text = when.describe();
    if file.job.iter().any(|job| job.when == when_text && job.prompt.eq_ignore_ascii_case(&prompt)) {
        return Ok(());
    }
    file.job.push(Job { id: file.next, when: when_text, prompt, enabled: true, last: starting(when, now) });
    file.next += 1;
    save(path, file)
}

/// The job with this id.
pub fn get(path: &Path, id: u32) -> Result<Job, String> {
    load(path)?.into_iter().find(|job| job.id == id).ok_or_else(|| "there is no such job".to_owned())
}

pub fn remove(path: &Path, id: u32) -> Result<(), String> {
    let mut file = read(path)?;
    let n = file.job.iter().position(|job| job.id == id).ok_or_else(|| "there is no such job".to_owned())?;
    file.job.remove(n);
    save(path, file)
}

/// Turns a job on or off. One turned on starts from now, so what was due while it was off doesn't run.
pub fn set_enabled(path: &Path, id: u32, on: bool, now: NaiveDateTime) -> Result<(), String> {
    let mut file = read(path)?;
    let job = file.job.iter_mut().find(|job| job.id == id).ok_or_else(|| "there is no such job".to_owned())?;
    if on && !job.enabled {
        job.last = When::parse(&job.when).ok().map(|when| starting(&when, now)).unwrap_or_default();
    }
    job.enabled = on;
    save(path, file)
}

/// The jobs that are due at `now`, each by its id with the time it was due: on, and due after it last ran. None while the
/// file can't be read: `Jobs` says why, and this is asked every half minute.
pub fn due(path: &Path, now: NaiveDateTime) -> Vec<(u32, NaiveDateTime)> {
    load(path)
        .unwrap_or_default()
        .iter()
        .filter(|job| job.enabled)
        .filter_map(|job| {
            let latest = When::parse(&job.when).ok()?.latest_due(now)?;
            let last = NaiveDateTime::parse_from_str(&job.last, FORMAT).ok();
            last.is_none_or(|last| latest > last).then_some((job.id, latest))
        })
        .collect()
}

/// Notes that the job with this id ran for the time it was due; nothing when it has been removed meanwhile.
pub fn ran(path: &Path, id: u32, due: NaiveDateTime) {
    let mut file = match read(path) {
        Ok(file) => file,
        Err(err) => return eprintln!("alfred: noting a job's run: {err}"),
    };
    if let Some(job) = file.job.iter_mut().find(|job| job.id == id) {
        job.last = due.format(FORMAT).to_string();
        if let Err(err) = save(path, file) {
            eprintln!("alfred: noting a job's run: {err}");
        }
    }
}

/// The jobs as JSON for a client: `[{"id","when","prompt","enabled","next","last"}]`, `next` as local time.
pub fn view(path: &Path, now: NaiveDateTime) -> Result<Value, String> {
    Ok(Value::Array(
        load(path)?
            .iter()
            .map(|job| {
                let next = job.enabled.then(|| When::parse(&job.when).ok().and_then(|when| when.next_due(now))).flatten().map(|next| next.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                json!({ "id": job.id, "when": job.when, "prompt": job.prompt, "enabled": job.enabled, "next": next, "last": job.last })
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn path(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("alfred-jobs-{test}-{}", std::process::id()));
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
        let jobs = load(&p).unwrap();
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
        assert_eq!(due(&p, at(20, 8, 0)), [(1, at(20, 8, 0))]);
        ran(&p, 1, at(20, 8, 0));
        assert!(due(&p, at(20, 8, 30)).is_empty());
        // The laptop sleeps through Wednesday and Thursday: on waking Friday at 10, it runs once, for Friday.
        assert_eq!(due(&p, at(23, 10, 0)), [(1, at(23, 8, 0))]);
        ran(&p, 1, at(23, 8, 0));
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
        set_enabled(&p, 1, false, at(19, 9, 0)).unwrap();
        assert!(due(&p, at(21, 9, 0)).is_empty());
        set_enabled(&p, 1, true, at(21, 9, 0)).unwrap();
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
        remove(&p, 1).unwrap();
        assert_eq!(load(&p).unwrap().len(), MAX_JOBS - 1);
        assert!(remove(&p, 9).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn jobs_toml_is_yours_only_and_says_its_version() {
        use std::os::unix::fs::PermissionsExt;
        let p = path("private");
        add(&p, &weekdays(8), "Summarise", at(19, 9, 0)).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(std::fs::read_to_string(&p).unwrap().contains("version = 1"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_jobs_toml_that_cant_be_understood_is_set_aside_not_written_over() {
        let p = path("bad");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "[[job]]\nwhen = \"weekdays at 8:00\"\nnot toml at all").unwrap();
        add(&p, &weekdays(9), "Summarise", at(19, 9, 0)).unwrap();
        assert_eq!(load(&p).unwrap().len(), 1);
        let aside: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().filter_map(|entry| entry.ok()).filter(|entry| entry.file_name().to_string_lossy().starts_with("jobs.toml.bad-")).collect();
        assert_eq!(aside.len(), 1);
        assert!(std::fs::read_to_string(aside[0].path()).unwrap().contains("not toml at all"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_jobs_toml_from_a_newer_version_is_left_as_it_is() {
        let p = path("newer");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let newer = "version = 99\n\n[[job]]\nwhen = \"weekdays at 8:00\"\nprompt = \"Summarise\"\nfuture = true\n";
        std::fs::write(&p, newer).unwrap();
        assert!(add(&p, &weekdays(9), "More", at(19, 9, 0)).unwrap_err().contains("newer"));
        assert!(due(&p, at(20, 8, 0)).is_empty());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), newer);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_job_keeps_its_id_when_another_is_removed() {
        let p = path("ids");
        for prompt in ["a", "b", "c"] {
            add(&p, &weekdays(8), &format!("job {prompt}"), at(19, 9, 0)).unwrap();
        }
        let ids: Vec<u64> = view(&p, at(19, 9, 0)).unwrap().as_array().unwrap().iter().map(|job| job["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [1, 2, 3]);
        remove(&p, 1).unwrap();
        // Job 3 is still job c, though it has moved up the list.
        set_enabled(&p, 3, false, at(19, 9, 0)).unwrap();
        ran(&p, 2, at(20, 8, 0));
        let jobs = load(&p).unwrap();
        assert_eq!(jobs.iter().map(|job| (job.prompt.as_str(), job.enabled)).collect::<Vec<_>>(), [("job b", true), ("job c", false)]);
        assert_eq!(jobs[0].last, "2026-10-20T08:00");
        assert!(remove(&p, 1).is_err());
        // An id is never given again, so a stale one reaches nothing.
        add(&p, &weekdays(8), "job d", at(19, 9, 0)).unwrap();
        assert_eq!(load(&p).unwrap().last().map(|job| job.id), Some(4));
        assert_eq!(due(&p, at(20, 8, 0)).iter().map(|(id, _)| *id).collect::<Vec<_>>(), [4]);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn jobs_written_before_ids_get_them_in_order() {
        let p = path("old");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "[[job]]\nwhen = \"weekdays at 8:00\"\nprompt = \"a\"\n\n[[job]]\nwhen = \"weekdays at 9:00\"\nprompt = \"b\"\n").unwrap();
        assert_eq!(load(&p).unwrap().iter().map(|job| job.id).collect::<Vec<_>>(), [1, 2]);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn the_view_has_what_a_client_shows() {
        let p = path("view");
        add(&p, &weekdays(8), "Summarise", at(19, 9, 0)).unwrap();
        let view = view(&p, at(19, 9, 0)).unwrap();
        assert_eq!((view[0]["id"].as_u64(), view[0]["when"].as_str(), view[0]["next"].as_str()), (Some(1), Some("weekdays at 8:00"), Some("2026-10-20 08:00")));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
