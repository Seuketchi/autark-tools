//! `autark-ai task <id>`: one run of a task, as its timer starts it. It reads the task's inputs, asks Ollama
//! for a report, saves it in the task's folder and sends the notification, all itself, so a task runs whether
//! or not the daemon is up. A task only reports: nothing here changes anything but its own folder.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::json;
use zbus::Variant;

use crate::config::Ai as Config;
use crate::ollama::Ollama;
use crate::{files, log, tasks, web};

#[zbus::proxy(interface = "org.freedesktop.Notifications", default_service = "org.freedesktop.Notifications", default_path = "/org/freedesktop/Notifications")]
pub trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(&self, app_name: &str, replaces_id: u32, app_icon: &str, summary: &str, body: &str, actions: &[&str], hints: HashMap<&str, Variant<'_>>, expire_timeout: i32) -> zbus::Result<u32>;
    fn close_notification(&self, id: u32) -> zbus::Result<()>;
}

/// How many past results a task keeps.
const KEPT_RUNS: usize = 30;

const SYSTEM: &str = "You are Kith, the assistant built into the Autark desktop, running one of the user's scheduled tasks. \
                      Do what the task says with the inputs given, and only report: you can't change anything, so say in words \
                      anything the user should do. If an input is missing or failed, say so. \
                      summary: two or three plain sentences for a notification. report: the full result, in Markdown. \
                      urgent: true only if the user should see it now rather than when they next look.";

pub fn run(id: &str) -> Result<(), String> {
    let config: Config = autark_common::config::load("ai");
    let task = tasks::load(id)?;
    let result = report(&config, id, &task);
    let delivered = async_io::block_on(deliver(&task, &result));
    if let Err(err) = delivered {
        eprintln!("ai: task {id}: sending its notification: {err}");
    }
    result.map(drop)
}

struct Report {
    summary: String,
    urgent: bool,
    /// The saved result.
    path: std::path::PathBuf,
    /// The first file it read, for the notification's second button.
    first_file: Option<std::path::PathBuf>,
}

fn report(config: &Config, id: &str, task: &tasks::Task) -> Result<Report, String> {
    if !config.enabled {
        return Err("AI is off in Settings".to_owned());
    }
    let (mut inputs, outcome) = files::read(&task.reads);
    for (path, why) in &outcome {
        if !why.is_empty() {
            inputs.push_str(&format!("File {path} couldn't be read: {why}\n\n"));
        }
    }
    for url in &task.urls {
        match web::fetch(url) {
            Ok(text) => inputs.push_str(&format!("Page {url}:\n{text}\n\n")),
            Err(why) => inputs.push_str(&format!("Page {url} couldn't be fetched: {why}\n\n")),
        }
    }
    let user = if inputs.is_empty() { format!("Task: {}", task.prompt) } else { format!("Task: {}\n\nInputs:\n{}", task.prompt, inputs.trim_end()) };
    let schema = json!({
        "type": "object",
        "properties": { "summary": { "type": "string" }, "report": { "type": "string" }, "urgent": { "type": "boolean" } },
        "required": ["summary", "report", "urgent"],
    });
    let start = Instant::now();
    let answer = Ollama::new(&config.ollama).json(SYSTEM, &user, &schema);
    log::record("task", "ollama", json!({ "task": task.name, "prompt": task.prompt, "reads": task.reads, "urls": task.urls }), &answer, start.elapsed());
    let answer = answer?;
    let summary = answer["summary"].as_str().unwrap_or_default().trim().to_owned();
    let body = answer["report"].as_str().map(str::trim).filter(|text| !text.is_empty()).unwrap_or(&summary).to_owned();
    let path = save(id, task, &body)?;
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let first_file = outcome.iter().find(|(_, why)| why.is_empty()).map(|(path, _)| path.strip_prefix("~/").map_or_else(|| path.into(), |rest| home.join(rest)));
    Ok(Report { summary, urgent: answer["urgent"].as_bool() == Some(true), path, first_file })
}

/// Saves the result as `<date>-<time>.md` in the task's folder, and as `latest.md`, keeping the last few runs.
fn save(id: &str, task: &tasks::Task, report: &str) -> Result<std::path::PathBuf, String> {
    let id_dir = tasks::folder().join(id);
    let runs = id_dir.join("runs");
    std::fs::create_dir_all(&runs).map_err(|err| format!("{}: {err}", runs.display()))?;
    let now = chrono::Local::now();
    let text = format!("# {}\n\n_{}_\n\n{report}\n", task.name, now.format("%A %-d %B %Y, %H:%M"));
    let run = runs.join(format!("{}.md", now.format("%Y-%m-%d-%H%M%S")));
    std::fs::write(&run, &text).map_err(|err| format!("{}: {err}", run.display()))?;
    let latest = id_dir.join("latest.md");
    std::fs::write(&latest, &text).map_err(|err| format!("{}: {err}", latest.display()))?;
    let mut old: Vec<_> = std::fs::read_dir(&runs).map(|entries| entries.flatten().map(|entry| entry.path()).collect()).unwrap_or_default();
    old.sort();
    for path in old.iter().rev().skip(KEPT_RUNS) {
        let _ = std::fs::remove_file(path);
    }
    Ok(latest)
}

/// The result as a card in the notification centre, a banner only if it's urgent; or why the task didn't run.
async fn deliver(task: &tasks::Task, result: &Result<Report, String>) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let proxy = NotificationsProxy::new(&connection).await?;
    let mut hints: HashMap<&str, Variant<'_>> = HashMap::new();
    let mut actions: Vec<String> = Vec::new();
    let (summary, body) = match result {
        Ok(report) => {
            let mut opens = HashMap::new();
            opens.insert("open".to_owned(), uri(&report.path));
            actions.extend(["open".to_owned(), "Open summary".to_owned()]);
            if let Some(file) = &report.first_file {
                opens.insert("file".to_owned(), uri(file));
                let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                actions.extend(["file".to_owned(), format!("Open {name}")]);
            }
            hints.insert("x-autark-open", Variant::from(opens));
            hints.insert("x-autark-quiet", Variant::from(!report.urgent));
            (task.name.clone(), format!("{}\n\nRan on this laptop with Ollama.", report.summary))
        }
        Err(why) => (task.name.clone(), format!("This task didn't run: {why}.")),
    };
    hints.insert("desktop-entry", Variant::from("autark-ai"));
    let actions: Vec<&str> = actions.iter().map(String::as_str).collect();
    proxy.notify("Kith", 0, "", &summary, &body, &actions, hints, -1).await.map(drop)
}

fn uri(path: &std::path::Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => uri.push(byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_uris() {
        assert_eq!(uri(std::path::Path::new("/home/tan/My Notes/log #2.md")), "file:///home/tan/My%20Notes/log%20%232.md");
    }
}
