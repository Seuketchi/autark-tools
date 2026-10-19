//! Kith's tool loop, for a question that needs more than one look: the model picks a read-only tool, code runs it,
//! the result goes back to the model, and so on for a few steps, then Kith answers from what was found. The tools
//! only read: the laptop's numbers, what Kith remembers, a file Kith may read (see `files`), the web. Nothing here
//! changes the laptop; actions stay on the cards that ask first. Each step is logged like any call and shown as a
//! line at the top of the answer, so what Kith looked at is never hidden.
//!
//! The model only decides, one small JSON answer a step. Code owns the rest: which tools exist, the step limit,
//! repeats, and the one rule that keeps files private: once Kith has read a file, it no longer searches the web,
//! because a search query leaves the laptop and could carry the file's words with it.

use serde_json::{Value, json};

use crate::{files, memory, senses, web};

/// How many tools a question may use. Each step is a model call, seconds on this laptop.
pub const MAX_STEPS: usize = 4;
/// How much of one tool's result the model sees.
const KEPT: usize = 1500;

/// What the loop found, one entry per step.
#[derive(Debug, PartialEq, Clone)]
pub struct Note {
    pub tool: String,
    pub input: String,
    pub result: String,
}

/// Whether the question is one for the loop: it needs two kinds of source (the laptop and the web, say), asks for
/// one thing after another, or compares. A question about one thing is answered in one go, as before.
pub fn wants_agent(question: &str) -> bool {
    let text = format!(" {} ", question.to_lowercase());
    senses::subjects(question).len() >= 2 || [" then ", " compare ", " and also ", " after that "].iter().any(|word| text.contains(word))
}

/// The tools open to the next step, in the order the model is told of them.
fn open(web: bool, read_a_file: bool) -> Vec<&'static str> {
    let mut tools = vec!["system", "memory", "read_file"];
    if web && !read_a_file {
        tools.push("web");
    }
    tools
}

fn describe(tool: &str) -> &'static str {
    match tool {
        "system" => "system: this laptop's disk, battery, memory and uptime now (input empty)",
        "memory" => "memory: what the user told Kith to remember (input empty)",
        "read_file" => "read_file: a file in the user's home folder (input: its path, such as ~/notes/todo.md)",
        "web" => "web: a web search for news, releases or facts (input: the search words)",
        _ => "",
    }
}

fn system_prompt(tools: &[&str]) -> String {
    let list: Vec<&str> = tools.iter().map(|tool| describe(tool)).collect();
    format!(
        "You help Kith, an assistant on the user's laptop, gather what it needs to answer one question. Each turn pick the next tool, or \"answer\" \
         when what's been found is enough or no tool can help. Never pick a tool whose result is already below, and don't use a tool the question \
         doesn't need.\nTools:\n- {}\n- answer: stop and answer (input empty)",
        list.join("\n- ")
    )
}

fn schema(tools: &[&str]) -> Value {
    let mut names: Vec<&str> = tools.to_vec();
    names.push("answer");
    json!({
        "type": "object",
        "properties": { "tool": { "enum": names }, "input": { "type": "string" } },
        "required": ["tool", "input"],
    })
}

fn user(question: &str, notes: &[Note]) -> String {
    if notes.is_empty() {
        return format!("Question: {question}\n\nFound so far: nothing.");
    }
    let found: Vec<String> = notes.iter().enumerate().map(|(n, note)| format!("{}. {} {}\n{}", n + 1, note.tool, note.input, note.result)).collect();
    format!("Question: {question}\n\nFound so far:\n{}", found.join("\n\n"))
}

fn cut(text: &str) -> String {
    if text.chars().count() <= KEPT {
        return text.to_owned();
    }
    format!("{} …", text.chars().take(KEPT).collect::<String>().trim_end())
}

/// What the line at the top of the answer says about a step.
pub fn shown(note: &Note) -> String {
    match note.tool.as_str() {
        "system" => "• looked at this laptop".to_owned(),
        "memory" => "• checked what I remember".to_owned(),
        "read_file" => format!("• read {}", note.input),
        "web" => format!("• searched the web for “{}”", note.input),
        other => format!("• {other}"),
    }
}

/// Runs the loop. `decide` is one model call (system prompt, user text, schema) and `run` one tool (name, input);
/// `shown_step` is told each step as it starts and says whether to go on (false when the user pressed Stop).
/// Returns what was found, which may be nothing, and whether it was stopped.
pub fn gather(
    question: &str,
    web: bool,
    mut decide: impl FnMut(&str, &str, &Value) -> Result<Value, String>,
    mut run: impl FnMut(&str, &str) -> Result<String, String>,
    mut shown_step: impl FnMut(&Note) -> bool,
) -> (Vec<Note>, bool) {
    let mut notes: Vec<Note> = Vec::new();
    let mut read_a_file = false;
    for _ in 0..MAX_STEPS {
        let tools = open(web, read_a_file);
        let choice = match decide(&system_prompt(&tools), &user(question, &notes), &schema(&tools)) {
            Ok(choice) => choice,
            Err(why) => {
                eprintln!("ai: the tool loop's step: {why}");
                break;
            }
        };
        let tool = choice["tool"].as_str().unwrap_or("answer");
        if !tools.contains(&tool) {
            break;
        }
        let input: String = choice["input"].as_str().unwrap_or_default().trim().chars().take(200).collect();
        if notes.iter().any(|note| note.tool == tool && note.input == input) {
            break;
        }
        let mut note = Note { tool: tool.to_owned(), input, result: String::new() };
        if !shown_step(&note) {
            return (notes, true);
        }
        let result = run(tool, &note.input);
        if tool == "read_file" && result.is_ok() {
            read_a_file = true;
        }
        note.result = match result {
            Ok(text) if text.trim().is_empty() => "(nothing)".to_owned(),
            Ok(text) => cut(&text),
            Err(why) => format!("failed: {why}"),
        };
        notes.push(note);
    }
    (notes, false)
}

/// What the answer is given besides the question: the notes, or why there are none.
pub fn found(notes: &[Note]) -> String {
    if notes.is_empty() {
        return "Nothing was looked up for this question.".to_owned();
    }
    let found: Vec<String> = notes.iter().map(|note| format!("{} {}:\n{}", note.tool, note.input, note.result)).collect();
    format!("What Kith looked up for this question (answer only from this and what you remember; if it doesn't cover the question, say what is missing):\n\n{}", found.join("\n\n"))
}

/// Runs one tool for real. Only reads; the web is only open when the caller says so.
pub fn execute(tool: &str, input: &str, web_open: bool) -> Result<String, String> {
    match tool {
        "system" => Ok(senses::system()),
        "memory" => {
            let facts = memory::facts();
            Ok(if facts.is_empty() { "Nothing is remembered yet.".to_owned() } else { facts.iter().map(|fact| format!("- {fact}")).collect::<Vec<_>>().join("\n") })
        }
        "read_file" => {
            let (text, outcome) = files::read(&[input.to_owned()]);
            match outcome.first() {
                Some((_, reason)) if !reason.is_empty() => Err(reason.clone()),
                _ => Ok(text),
            }
        }
        "web" if web_open && !input.is_empty() => async_io::block_on(async {
            let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
            let key = web::key(&connection).await?;
            let query = input.to_owned();
            blocking::unblock(move || web::lookup(&key, &query)).await
        }),
        "web" => Err("the web isn't open here, or there was nothing to search for".to_owned()),
        other => Err(format!("no tool “{other}”")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays a script of model choices, and records what ran.
    fn play(script: &[(&str, &str)], web: bool) -> (Vec<Note>, bool, Vec<String>) {
        let mut at = 0;
        let mut seen: Vec<String> = Vec::new();
        let mut ran: Vec<String> = Vec::new();
        let (notes, stopped) = gather(
            "is my disk ok and what's the newest kernel",
            web,
            |_, _, schema| {
                let (tool, input) = script.get(at).copied().unwrap_or(("answer", ""));
                at += 1;
                // The schema only offers open tools.
                let offered: Vec<&str> = schema["properties"]["tool"]["enum"].as_array().unwrap().iter().filter_map(|tool| tool.as_str()).collect();
                seen.push(offered.join(","));
                Ok(json!({ "tool": tool, "input": input }))
            },
            |tool, input| {
                ran.push(format!("{tool}:{input}"));
                Ok(format!("{tool} result"))
            },
            |_| true,
        );
        ran.extend(seen.into_iter().map(|offered| format!("offered {offered}")));
        (notes, stopped, ran)
    }

    #[test]
    fn it_takes_two_looks_then_answers() {
        let (notes, stopped, ran) = play(&[("system", ""), ("web", "newest linux kernel"), ("answer", "")], true);
        assert!(!stopped);
        assert_eq!(notes.iter().map(|note| note.tool.as_str()).collect::<Vec<_>>(), ["system", "web"]);
        assert_eq!(&ran[..2], ["system:", "web:newest linux kernel"]);
        assert_eq!(notes[1].result, "web result");
    }

    #[test]
    fn it_stops_at_the_step_limit_and_at_a_repeat() {
        let (notes, _, _) = play(&[("system", ""), ("memory", ""), ("system", ""), ("memory", "")], true);
        assert_eq!(notes.len(), 2, "a repeated tool and input ends the loop");
        let many = [("system", ""), ("memory", ""), ("read_file", "~/a"), ("read_file", "~/b"), ("read_file", "~/c")];
        let (notes, _, _) = play(&many, false);
        assert_eq!(notes.len(), MAX_STEPS);
    }

    #[test]
    fn a_tool_that_is_not_open_ends_the_loop() {
        let (notes, _, _) = play(&[("web", "anything")], false);
        assert!(notes.is_empty(), "the web wasn't offered");
        let (notes, _, _) = play(&[("calendar", "")], true);
        assert!(notes.is_empty(), "there is no calendar tool");
    }

    #[test]
    fn after_a_file_is_read_the_web_is_closed() {
        let (notes, _, ran) = play(&[("read_file", "~/notes/todo.md"), ("web", "what the file said")], true);
        assert_eq!(notes.len(), 1, "a search could carry the file's words off the laptop");
        let offered: Vec<&String> = ran.iter().filter(|line| line.starts_with("offered")).collect();
        assert_eq!(offered[0], "offered system,memory,read_file,web,answer");
        assert_eq!(offered[1], "offered system,memory,read_file,answer");
    }

    #[test]
    fn a_failed_tool_is_a_note_and_the_user_can_stop_it() {
        let mut calls = 0;
        let (notes, stopped) = gather("q", false, |_, _, _| Ok(json!({ "tool": "read_file", "input": "~/.ssh/id_ed25519" })), |_, _| Err("Kith doesn't read dotfiles".to_owned()), |_| true);
        assert_eq!(notes[0].result, "failed: Kith doesn't read dotfiles");
        assert!(!stopped);
        let (notes, stopped) = gather("q", false, |_, _, _| Ok(json!({ "tool": "system", "input": "" })), |_, _| Ok(String::new()), |_| {
            calls += 1;
            false
        });
        assert!((notes.is_empty(), stopped, calls) == (true, true, 1));
    }

    #[test]
    fn a_model_that_fails_ends_the_loop_with_what_it_has() {
        let mut first = true;
        let (notes, stopped) = gather(
            "q",
            true,
            |_, _, _| if std::mem::take(&mut first) { Ok(json!({ "tool": "system", "input": "" })) } else { Err("timeout".to_owned()) },
            |_, _| Ok("disk 80%".to_owned()),
            |_| true,
        );
        assert_eq!((notes.len(), stopped), (1, false));
        assert!(found(&notes).contains("disk 80%"));
        assert_eq!(found(&[]), "Nothing was looked up for this question.");
    }

    #[test]
    fn only_questions_that_need_more_than_one_look_use_the_loop() {
        assert!(wants_agent("how much disk is left and what's the latest kernel news"));
        assert!(wants_agent("check my battery then tell me the news"));
        assert!(!wants_agent("how much disk is left"));
        assert!(!wants_agent("why is the sky blue"));
    }

    #[test]
    fn long_results_are_cut_and_steps_are_shown() {
        assert!(cut(&"x".repeat(5000)).chars().count() < 1600);
        let note = Note { tool: "web".into(), input: "kernel".into(), result: String::new() };
        assert_eq!(shown(&note), "• searched the web for “kernel”");
    }
}
