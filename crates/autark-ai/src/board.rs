//! Kith's task board: what is on your GitHub Projects, read with the GitHub CLI (`gh`) that is already signed in on
//! this laptop. Kith holds no GitHub token: `gh` keeps its own, and only needs the read-only `read:project`
//! permission (`gh auth refresh -h github.com -s read:project`). Read-only: nothing is created, moved or closed.
//!
//! Every open project of yours is listed. "Today" is what is in progress, meaning any status that is not Todo or
//! Done, then the first few Todo items; Done items are only counted. A question that is only about tasks is
//! answered from the board by code, not by the model. Issue titles can be private, so the loop closes the web after a
//! board read, as after any private read.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The most Todo items listed under a project.
const NEXT: usize = 5;
/// The most items of one project that are looked at.
const LOOKED_AT: &str = "100";
/// How long one `gh` call may take.
const WAIT: Duration = Duration::from_secs(25);

/// One item on a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub title: String,
    pub status: String,
    /// `owner/repo#12`, or empty for a draft item.
    pub reference: String,
}

impl Item {
    fn is_done(&self) -> bool {
        matches!(self.status.to_lowercase().as_str(), "done" | "closed" | "complete" | "completed" | "shipped")
    }

    fn is_todo(&self) -> bool {
        matches!(self.status.to_lowercase().as_str(), "" | "todo" | "to do" | "backlog" | "new" | "triage")
    }
}

/// `gh`, wherever this session can find it.
fn gh() -> Command {
    let home = std::env::var("HOME").unwrap_or_default();
    let found = ["/usr/bin/gh".to_owned(), "/usr/local/bin/gh".to_owned(), format!("{home}/.local/bin/gh")].into_iter().find(|path| std::path::Path::new(path).exists());
    let mut command = Command::new(found.unwrap_or_else(|| "gh".to_owned()));
    command.env("GH_PROMPT_DISABLED", "1").env("GH_NO_UPDATE_NOTIFIER", "1").env("NO_COLOR", "1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

/// What `gh` printed for these arguments, or why it didn't print. Never carries a token: `gh` doesn't print one.
fn run(args: &[&str]) -> Result<String, String> {
    let mut child = gh().args(args).spawn().map_err(|err| if err.kind() == std::io::ErrorKind::NotFound { "the GitHub CLI (gh) isn't installed".to_owned() } else { format!("couldn't run gh: {err}") })?;
    let (mut out, mut err) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    let reader = std::thread::spawn(move || {
        let (mut text, mut why) = (String::new(), String::new());
        let _ = out.read_to_string(&mut text);
        let _ = err.read_to_string(&mut why);
        (text, why)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < WAIT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("GitHub took too long to answer".to_owned());
            }
        }
    };
    let (text, why) = reader.join().map_err(|_| "couldn't read gh's answer".to_owned())?;
    if status.success() { Ok(text) } else { Err(explain(&why)) }
}

/// A `gh` failure in words that say what to do.
fn explain(why: &str) -> String {
    let lower = why.to_lowercase();
    if lower.contains("read:project") || lower.contains("required scopes") {
        "GitHub hasn't been given the read:project permission (run: gh auth refresh -h github.com -s read:project)".to_owned()
    } else if lower.contains("auth login") || lower.contains("not logged") || lower.contains("authentication") {
        "the GitHub CLI isn't signed in (run: gh auth login)".to_owned()
    } else {
        let line = why.lines().find(|line| !line.trim().is_empty()).unwrap_or("it failed");
        format!("GitHub said: {}", line.chars().take(120).collect::<String>())
    }
}

/// The open projects in `gh project list`'s JSON: number and title.
pub fn projects(json: &str) -> Result<Vec<(u64, String)>, String> {
    let value: Value = serde_json::from_str(json).map_err(|_| "GitHub answered in a way Kith doesn't understand".to_owned())?;
    Ok(value
        .get("projects")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|project| !project.get("closed").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|project| Some((project.get("number")?.as_u64()?, project.get("title")?.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default())
}

/// The items in `gh project item-list`'s JSON, in the board's order. A body is never read.
pub fn items(json: &str) -> Result<Vec<Item>, String> {
    let value: Value = serde_json::from_str(json).map_err(|_| "GitHub answered in a way Kith doesn't understand".to_owned())?;
    Ok(value
        .get("items")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|item| {
                    let content = item.get("content");
                    let text = |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or("").to_owned();
                    let title = Some(text(item.get("title"))).filter(|title| !title.is_empty()).unwrap_or_else(|| text(content.and_then(|c| c.get("title"))));
                    let reference = match (content.and_then(|c| c.get("repository")).and_then(Value::as_str), content.and_then(|c| c.get("number")).and_then(Value::as_u64)) {
                        (Some(repo), Some(number)) => format!("{repo}#{number}"),
                        _ => String::new(),
                    };
                    Item { title: line(&title, 100), status: line(&text(item.get("status")), 30), reference: line(&reference, 80) }
                })
                .filter(|item| !item.title.is_empty())
                .collect()
        })
        .unwrap_or_default())
}

/// Text from GitHub shown as one plain line: no control characters, cut at a width.
fn line(text: &str, width: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= width {
        return flat;
    }
    format!("{}…", flat.chars().take(width).collect::<String>().trim_end())
}

/// One project as lines: what is in progress, the first Todo items, and how many are done.
pub fn describe(title: &str, items: &[Item]) -> String {
    let moving: Vec<&Item> = items.iter().filter(|item| !item.is_done() && !item.is_todo()).collect();
    let todo: Vec<&Item> = items.iter().filter(|item| item.is_todo()).collect();
    let done = items.iter().filter(|item| item.is_done()).count();
    let show = |item: &&Item, with_status: bool| {
        let status = if with_status { format!("{} — ", item.status) } else { String::new() };
        let reference = if item.reference.is_empty() { String::new() } else { format!(" ({})", item.reference) };
        format!("• {status}{}{reference}", item.title)
    };
    let mut lines = vec![format!("{title}:")];
    if moving.is_empty() && todo.is_empty() {
        lines.push(format!("Nothing open ({done} done)."));
        return lines.join("\n");
    }
    if !moving.is_empty() {
        lines.push("In progress:".to_owned());
        lines.extend(moving.iter().map(|item| show(item, true)));
    }
    if !todo.is_empty() {
        lines.push(if todo.len() > NEXT { format!("Todo (the first {NEXT} of {}):", todo.len()) } else { "Todo:".to_owned() });
        lines.extend(todo.iter().take(NEXT).map(|item| show(item, false)));
    }
    lines.push(format!("{done} done."));
    lines.join("\n")
}

/// Every open project of yours, as lines. Blocks while `gh` runs, so callers run it off the main thread.
pub fn answer() -> Result<String, String> {
    let list = projects(&run(&["project", "list", "--owner", "@me", "--format", "json", "--limit", "20"])?)?;
    if list.is_empty() {
        return Err("you have no open GitHub projects".to_owned());
    }
    let mut parts = Vec::new();
    for (number, title) in list {
        let number = number.to_string();
        let json = run(&["project", "item-list", &number, "--owner", "@me", "--format", "json", "--limit", LOOKED_AT])?;
        parts.push(describe(&line(&title, 60), &items(&json)?));
    }
    Ok(format!("Your GitHub projects, read just now:\n\n{}", parts.join("\n\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"projects":[{"number":3,"title":"Tern","closed":false},{"number":2,"title":"Old","closed":true},{"number":1,"title":"Udacity Journey","closed":false}]}"#;

    const ITEMS: &str = r#"{"totalCount":9,"items":[
        {"title":"Review the parser","status":"In review","content":{"type":"PullRequest","number":12,"repository":"me/tern","title":"Review the parser","body":"secret body"}},
        {"title":"Write docs","status":"Todo","content":{"type":"Issue","number":5,"repository":"me/tern"}},
        {"title":"Ship it","status":"Done","content":{"type":"Issue","number":4,"repository":"me/tern"}},
        {"title":"Draft idea","status":"Todo"},
        {"title":"Cook the stock","status":"Cooking","content":{"type":"Issue","number":9,"repository":"me/tern"}},
        {"title":"A\nmultiline\u0007 title","status":"Todo"},
        {"title":"Four","status":"Todo"},{"title":"Five","status":"Todo"},{"title":"Six","status":"Todo"}
    ]}"#;

    #[test]
    fn open_projects_are_listed_and_closed_ones_are_not() {
        assert_eq!(projects(LIST).unwrap(), vec![(3, "Tern".to_owned()), (1, "Udacity Journey".to_owned())]);
        assert!(projects("nope").is_err());
        assert!(projects("{}").unwrap().is_empty());
    }

    #[test]
    fn a_project_shows_what_is_moving_then_the_first_todo_items() {
        let text = describe("Tern", &items(ITEMS).unwrap());
        assert_eq!(
            text,
            "Tern:\nIn progress:\n• In review — Review the parser (me/tern#12)\n• Cooking — Cook the stock (me/tern#9)\nTodo (the first 5 of 6):\n• Write docs (me/tern#5)\n• Draft idea\n• A multiline title\n• Four\n• Five\n1 done."
        );
        assert!(!text.contains("secret body"), "a body is never read");
    }

    #[test]
    fn a_finished_project_says_so() {
        let done = items(r#"{"items":[{"title":"x","status":"Done"},{"title":"y","status":"Done"}]}"#).unwrap();
        assert_eq!(describe("Old", &done), "Old:\nNothing open (2 done).");
        assert_eq!(describe("Empty", &[]), "Empty:\nNothing open (0 done).");
    }

    #[test]
    fn a_failure_says_what_to_do() {
        assert!(explain("error: your authentication token is missing required scopes [read:project]").contains("gh auth refresh"));
        assert!(explain("To get started with GitHub CLI, please run:  gh auth login").contains("gh auth login"));
        assert_eq!(explain("\n\nsomething odd happened"), "GitHub said: something odd happened");
    }
}
