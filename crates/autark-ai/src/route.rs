//! `Route`: a sentence typed into the launcher becomes the actions it asks for, each with how sure the model
//! is, and the question it asks, with the files that question needs. "Dim the screen and mute, then tell me why
//! my last build stopped" is two actions and a question. It also proposes a lasting fact about you to remember,
//! which is kept only if you save it, and a task when the sentence asks for something on a schedule ("every
//! morning at 8, summarise last night's build log"), created only once you confirm its card. The model only picks actions from the catalog below and files from the
//! ones named or remembered; code checks every argument, and `Act` runs the actions you choose.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Debug, PartialEq)]
pub struct Routed {
    pub actions: Vec<Chosen>,
    /// What's asked, put as a question on its own; empty when the sentence only asks for actions.
    pub question: String,
    /// Files the question needs, as `~/…`, from the ones the sentence names or a memory points to.
    pub files: Vec<String>,
    /// A lasting fact about you to propose remembering: one you asked Kith to remember, or one it noticed.
    /// Empty when there's none; it's saved only if you say so.
    pub remember: String,
    /// A task to propose, with the files picked for it and the https pages the sentence names.
    pub task: Option<crate::tasks::Task>,
    /// How sure the model is of the whole, 0 to 1.
    pub confidence: f64,
    /// Who answered: `ollama`, or `jev` once its client is built.
    pub engine: &'static str,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Chosen {
    #[serde(flatten)]
    pub action: Action,
    /// How sure the model is that you asked for this one; the launcher leaves an unsure one unticked.
    pub confidence: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    SetVolume {
        percent: u8,
    },
    Mute {
        on: bool,
    },
    SetBrightness {
        percent: u8,
    },
    Wifi {
        on: bool,
    },
    Focus {
        on: bool,
    },
    NightLight {
        on: bool,
    },
    Lock,
    Sleep,
    OpenApp {
        name: String,
    },
    /// Only a suggestion offers these three; they aren't in the catalog the model picks from.
    /// Asks an app you started to quit, by its cgroup under `app.slice`.
    QuitApp {
        app: String,
    },
    /// Opens a file in its default app.
    OpenFile {
        path: String,
    },
    /// Opens a pane of Settings.
    OpenSettings {
        pane: String,
    },
}

const CATALOG: [(&str, &str); 9] = [
    ("set_volume", "set the sound volume (number: percent)"),
    ("mute", "mute or unmute the sound (on: true mutes)"),
    ("set_brightness", "set the screen brightness (number: percent)"),
    ("wifi", "turn Wi-Fi on or off"),
    ("focus", "turn Focus (do not disturb) on or off"),
    ("night_light", "turn Night light on or off"),
    ("lock", "lock the screen"),
    ("sleep", "put the laptop to sleep"),
    ("open_app", "open an app (text: its name)"),
];

/// The instructions, with what Kith remembers about you, which is how "my last build" finds its log.
pub fn system_prompt(facts: &[String]) -> String {
    let actions: Vec<String> = CATALOG.iter().map(|(name, what)| format!("- {name}: {what}")).collect();
    let mut prompt = format!(
        "You turn one sentence typed into a desktop launcher into JSON. A sentence can ask for actions, ask a question, or both.\n\
         actions: each change the sentence asks for that an action below can make, with its argument and your confidence from 0 to 1 \
         that it was asked for. None if it asks for no change.\n\
         question: the part that asks something to be answered or explained, put as a question on its own. Empty if there's none.\n\
         files: the files from the list given that are needed to answer the question. None if no list is given.\n\
         remember: a lasting fact about the user worth keeping, put briefly as a note about them (\"Works on Autark in the evenings\"): \
         what they ask you to remember, or something lasting they say about themselves, their work, their setup or their preferences. \
         Empty if there's none, if it's already remembered, or if it's a password, key, code or anything secret.\n\
         task: only when the sentence asks for something to be done again and again on a schedule: a short name, the instruction to \
         carry out each time (prompt), every (day, weekdays, weekends, a weekday such as monday, or month), day (of the month, for month) \
         and time (24-hour, HH:MM). The files it needs go in files. Leave task out otherwise, and then leave the question empty.\n\
         confidence: how sure you are of the whole answer, from 0 to 1.\n\
         Actions:\n{}",
        actions.join("\n")
    );
    if !facts.is_empty() {
        let facts: Vec<String> = facts.iter().map(|fact| format!("- {fact}")).collect();
        prompt.push_str(&format!("\nWhat you remember about the user:\n{}", facts.join("\n")));
    }
    prompt
}

/// The answer's shape; `files` can only be among `candidates`.
pub fn schema(candidates: &[String]) -> Value {
    let names: Vec<&str> = CATALOG.iter().map(|(name, ..)| *name).collect();
    let mut properties = json!({
        "actions": { "type": "array", "items": {
            "type": "object",
            "properties": {
                "name": { "enum": names },
                "number": { "type": "number" },
                "on": { "type": "boolean" },
                "text": { "type": "string" },
                "confidence": { "type": "number" },
            },
            "required": ["name", "confidence"],
        }},
        "question": { "type": "string" },
        "remember": { "type": "string" },
        "task": {
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "prompt": { "type": "string" },
                "every": { "enum": ["day", "weekdays", "weekends", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "month"] },
                "day": { "type": "number" },
                "time": { "type": "string" },
            },
            "required": ["name", "prompt", "every", "time"],
        },
        "confidence": { "type": "number" },
    });
    if !candidates.is_empty() {
        properties["files"] = json!({ "type": "array", "items": { "enum": candidates } });
    }
    json!({ "type": "object", "properties": properties, "required": ["actions", "question", "remember", "confidence"] })
}

/// What the model is told besides the instructions: the conversation so far if there is one, the sentence,
/// and the files it may pick from.
pub fn user(text: &str, candidates: &[String], conversation: &str) -> String {
    let mut user = if conversation.is_empty() { text.to_owned() } else { format!("Conversation so far:\n{conversation}\n\nNew message: {text}") };
    if !candidates.is_empty() {
        user.push_str(&format!("\n\nFiles: {}", candidates.join(", ")));
    }
    user
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    actions: Vec<RawAction>,
    #[serde(default)]
    question: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    remember: String,
    task: Option<RawTask>,
    confidence: f64,
}

#[derive(Deserialize)]
struct RawTask {
    #[serde(default)]
    name: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    every: String,
    #[serde(default)]
    day: f64,
    #[serde(default)]
    time: String,
}

#[derive(Deserialize)]
struct RawAction {
    name: String,
    number: Option<f64>,
    on: Option<bool>,
    text: Option<String>,
    confidence: Option<f64>,
}

/// Checks the model's answer against the catalog, the candidate files and the facts already kept. With no
/// actions, question or fact, the whole sentence is the question: you asked Kith, so Kith answers.
pub fn parse(answer: Value, text: &str, candidates: &[String], facts: &[String]) -> Result<Routed, String> {
    let answer: Answer = serde_json::from_value(answer).map_err(|err| format!("the route doesn't fit the schema: {err}"))?;
    let confidence = answer.confidence.clamp(0.0, 1.0);
    let actions = answer
        .actions
        .into_iter()
        .map(|raw| {
            let sure = raw.confidence.unwrap_or(confidence).clamp(0.0, 1.0);
            action(raw).map(|action| Chosen { action, confidence: sure })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut remember: String = answer.remember.trim().chars().take(200).collect();
    if facts.iter().any(|fact| fact.eq_ignore_ascii_case(&remember)) {
        remember.clear();
    }
    let mut files: Vec<String> = Vec::new();
    for file in answer.files {
        if candidates.contains(&file) && !files.contains(&file) {
            files.push(file);
        }
    }
    // A task asks for later, so nothing is answered now; its files are the task's.
    let task = match answer.task.filter(|task| !task.name.trim().is_empty() && !task.prompt.trim().is_empty()) {
        Some(raw) => {
            let when = crate::tasks::When { every: raw.every, day: raw.day.clamp(0.0, 31.0) as u8, time: raw.time };
            let task = crate::tasks::Task { name: raw.name, prompt: raw.prompt, when, reads: std::mem::take(&mut files), urls: crate::web::named(text) };
            Some(crate::tasks::check(task).map_err(|why| format!("the task doesn't fit: {why}"))?)
        }
        None => None,
    };
    let mut question = if task.is_some() { String::new() } else { answer.question.trim().to_owned() };
    if actions.is_empty() && question.is_empty() && remember.is_empty() && task.is_none() {
        question = text.trim().to_owned();
    }
    if question.is_empty() {
        files.clear();
    }
    Ok(Routed { actions, question, files, remember, task, confidence, engine: "ollama" })
}

/// The files the sentence names, then the ones memories point to.
pub fn candidates(text: &str, facts: &[String]) -> Vec<String> {
    let mut found = crate::files::named(text);
    for file in facts.iter().flat_map(|fact| crate::files::named(fact)) {
        if !found.contains(&file) {
            found.push(file);
        }
    }
    found
}

fn action(raw: RawAction) -> Result<Action, String> {
    let Some((name, _)) = CATALOG.iter().find(|(name, ..)| *name == raw.name) else {
        return Err(format!("unknown action {:?}", raw.name));
    };
    let missing = |what: &str| format!("{name} needs {what}");
    let percent = || raw.number.filter(|n| n.is_finite()).map(|n| n.round().clamp(0.0, 100.0) as u8).ok_or_else(|| missing("a number"));
    let on = || raw.on.ok_or_else(|| missing("on"));
    let text = || raw.text.clone().filter(|t| !t.trim().is_empty()).ok_or_else(|| missing("text"));
    Ok(match *name {
        "set_volume" => Action::SetVolume { percent: percent()? },
        "mute" => Action::Mute { on: on()? },
        "set_brightness" => Action::SetBrightness { percent: percent()? },
        "wifi" => Action::Wifi { on: on()? },
        "focus" => Action::Focus { on: on()? },
        "night_light" => Action::NightLight { on: on()? },
        "lock" => Action::Lock,
        "sleep" => Action::Sleep,
        _ => Action::OpenApp { name: text()? },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_and_a_question_from_one_sentence() {
        let answer = json!({ "confidence": 0.9, "question": "Why did my last build stop?", "files": ["~/autark/log.md"], "actions": [
            { "name": "set_brightness", "number": 30, "confidence": 0.94 }, { "name": "mute", "on": true, "confidence": 0.97 } ] });
        let candidates = vec!["~/autark/log.md".to_owned()];
        let routed = parse(answer, "dim the screen and mute, then tell me why my last build stopped", &candidates, &[]).unwrap();
        assert_eq!(routed.actions, vec![Chosen { action: Action::SetBrightness { percent: 30 }, confidence: 0.94 }, Chosen { action: Action::Mute { on: true }, confidence: 0.97 }]);
        assert_eq!(routed.question, "Why did my last build stop?");
        assert_eq!(routed.files, candidates);
        let json = serde_json::to_value(&routed).unwrap();
        assert_eq!(json["actions"][0], json!({ "action": "set_brightness", "percent": 30, "confidence": 0.94 }));
        assert_eq!(json["actions"][1], json!({ "action": "mute", "on": true, "confidence": 0.97 }));
        assert_eq!(json["engine"], "ollama");
    }

    #[test]
    fn arguments_and_files_are_checked() {
        let clamp = json!({ "confidence": 2.0, "question": "", "actions": [{ "name": "set_volume", "number": 140, "confidence": 0.8 }, { "name": "lock", "confidence": 0.5 }] });
        let routed = parse(clamp, "", &[], &[]).unwrap();
        assert_eq!(routed.confidence, 1.0);
        assert_eq!(routed.actions[0].action, Action::SetVolume { percent: 100 });
        assert_eq!(serde_json::to_value(&routed.actions[1]).unwrap(), json!({ "action": "lock", "confidence": 0.5 }));
        assert!(parse(json!({ "confidence": 1, "actions": [{ "name": "wifi", "confidence": 1 }] }), "", &[], &[]).is_err());
        assert!(parse(json!({ "confidence": 1, "actions": [{ "name": "rm_rf", "confidence": 1 }] }), "", &[], &[]).is_err());
        let invented = json!({ "confidence": 1, "question": "what's in it?", "files": ["~/autark/log.md", "/etc/shadow"] });
        assert_eq!(parse(invented, "", &["~/autark/log.md".to_owned()], &[]).unwrap().files, vec!["~/autark/log.md"]);
    }

    #[test]
    fn nothing_routed_asks_the_whole_sentence() {
        let routed = parse(json!({ "confidence": 0.4, "question": " ", "actions": [] }), "how much swap do I have ", &[], &[]).unwrap();
        assert_eq!(routed.question, "how much swap do I have");
        assert!(routed.actions.is_empty());
    }

    #[test]
    fn a_schedule_is_a_task() {
        let answer = json!({ "confidence": 0.9, "question": "What's in the log?", "actions": [], "remember": "", "files": ["~/autark/log.md"],
            "task": { "name": "Morning build summary", "prompt": "Summarise last night's build log", "every": "day", "time": "08:00" } });
        let text = "every morning at 8, summarise last night's build log and https://example.org/news";
        let routed = parse(answer, text, &["~/autark/log.md".to_owned()], &[]).unwrap();
        let task = routed.task.unwrap();
        assert_eq!(task.when.calendar().unwrap(), "*-*-* 08:00:00");
        assert_eq!(task.reads, vec!["~/autark/log.md"]);
        assert_eq!(task.urls, vec!["https://example.org/news"]);
        assert!(routed.question.is_empty() && routed.files.is_empty());
        let wrong = json!({ "confidence": 0.9, "question": "", "actions": [], "task": { "name": "x", "prompt": "y", "every": "day", "time": "8pm" } });
        assert!(parse(wrong, "", &[], &[]).is_err());
    }

    #[test]
    fn a_fact_to_remember_is_no_question() {
        let answer = json!({ "confidence": 0.9, "question": "", "actions": [], "remember": " Works on Autark in the evenings " });
        let routed = parse(answer.clone(), "remember that I work on Autark in the evenings", &[], &[]).unwrap();
        assert_eq!(routed.remember, "Works on Autark in the evenings");
        assert!(routed.question.is_empty());
        let kept = parse(answer, "remember that I work on Autark in the evenings", &[], &["works on autark in the evenings".to_owned()]).unwrap();
        assert!(kept.remember.is_empty());
        assert_eq!(kept.question, "remember that I work on Autark in the evenings");
    }
}
