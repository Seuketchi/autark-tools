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
    /// Who answered: `ollama` or `jev`.
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

/// Whether the sentence asks for something on a schedule. Only then is the model offered `task`: a small model
/// handed the field fills it in for sentences that never asked, and every extra field costs generated tokens.
fn wants_task(text: &str) -> bool {
    let text = text.to_lowercase();
    ["every ", "each ", "daily", "weekly", "monthly", "nightly", "hourly", "weekday", "weekend", "mondays", "tuesdays", "wednesdays", "thursdays", "fridays", "saturdays", "sundays"]
        .iter()
        .any(|word| text.contains(word))
}

/// Whether the sentence asks to remember something or says something lasting about the user; only then is the
/// model offered `remember`, for the same reasons as `task`.
fn wants_remember(text: &str) -> bool {
    let text = text.to_lowercase();
    ["remember", "keep in mind", "don't forget", "note that", "i am ", "i'm ", "i work", "i use ", "i prefer", "i like", "i live", "my name"]
        .iter()
        .any(|word| text.contains(word))
}

/// The instructions, with what Kith remembers about you, which is how "my last build" finds its log. `text` is
/// the new sentence: the instructions cover `remember` and `task` only when it could mean them.
pub fn system_prompt(facts: &[String], text: &str) -> String {
    let actions: Vec<String> = CATALOG.iter().map(|(name, what)| format!("- {name}: {what}")).collect();
    let mut prompt = String::from(
        "You turn one sentence typed into a desktop launcher into JSON. A sentence can ask for actions, ask a question, or both.\n\
         actions: each change the sentence asks for that an action below can make, with its argument and your confidence from 0 to 1 \
         that it was asked for. None if it asks for no change.\n\
         question: the part that asks something to be answered or explained, put as a question on its own. Empty if there's none.\n\
         files: the files from the list given that are needed to answer the question. None if no list is given.\n",
    );
    if wants_remember(text) {
        prompt.push_str(
            "remember: a lasting fact about the user worth keeping, put briefly as a note about them: what they ask you to remember, \
             or something lasting they say about themselves, their work, their setup or their preferences. Empty if there's none, if it's \
             already remembered, or if it's a password, key, code or anything secret.\n",
        );
    }
    if wants_task(text) {
        prompt.push_str(
            "task: the sentence asks for something to be done again and again on a schedule: a short name, the instruction to carry out \
             each time (prompt), every (day, weekdays, weekends, a weekday such as monday, or month), day (of the month, for month) and \
             time (24-hour, HH:MM). The files it needs go in files. Leave the question empty.\n",
        );
    }
    prompt.push_str("confidence: how sure you are of the whole answer, from 0 to 1.\n");
    prompt.push_str(&format!("Actions:\n{}\n", actions.join("\n")));
    prompt.push_str(
        "Examples (only actions the sentence asks for; none otherwise):\n\
         mute -> {\"actions\":[{\"name\":\"mute\",\"on\":true,\"confidence\":0.95}],\"question\":\"\",\"confidence\":0.95}\n\
         turn the volume down a bit and open files -> {\"actions\":[{\"name\":\"set_volume\",\"number\":30,\"confidence\":0.7},{\"name\":\"open_app\",\"text\":\"Files\",\"confidence\":0.95}],\"question\":\"\",\"confidence\":0.85}\n\
         why is the sky blue -> {\"actions\":[],\"question\":\"Why is the sky blue?\",\"confidence\":0.9}",
    );
    if !facts.is_empty() {
        let facts: Vec<String> = facts.iter().map(|fact| format!("- {fact}")).collect();
        prompt.push_str(&format!("\nWhat you remember about the user:\n{}", facts.join("\n")));
    }
    prompt
}

/// The answer's shape; `files` can only be among `candidates`, and `remember` and `task` are there only when the
/// sentence could mean them.
pub fn schema(candidates: &[String], text: &str) -> Value {
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
        "question": { "type": "string", "maxLength": 240 },
        "confidence": { "type": "number" },
    });
    let mut required = vec!["actions", "question"];
    if wants_remember(text) {
        properties["remember"] = json!({ "type": "string" });
        required.push("remember");
    }
    if wants_task(text) {
        properties["task"] = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "prompt": { "type": "string" },
                "every": { "enum": ["day", "weekdays", "weekends", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "month"] },
                "day": { "type": "number" },
                "time": { "type": "string" },
            },
            "required": ["name", "prompt", "every", "time"],
        });
    }
    required.push("confidence");
    if !candidates.is_empty() {
        properties["files"] = json!({ "type": "array", "items": { "enum": candidates } });
    }
    json!({ "type": "object", "properties": properties, "required": required })
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
    // A small model sometimes carries on after the question mark with its own answer and filler; the field is only the question.
    if let Some(end) = question.find('?') {
        question.truncate(end + 1);
    }
    if actions.is_empty() && question.is_empty() && remember.is_empty() && task.is_none() {
        question = text.trim().to_owned();
    }
    if question.is_empty() {
        files.clear();
    }
    Ok(Routed { actions, question, files, remember, task, confidence, engine: "ollama" })
}

/// What Jev is asked about each catalog action: whether the sentence asks for it, and for the ones with a
/// switch, which way (the noul is the chance of the first).
const JEV_ACTIONS: [(&str, &str, Option<&str>); 9] = [
    ("set_volume", "The user gives a sound volume level, or says to turn the sound up or down. Muting or unmuting alone is not this.", None),
    ("mute", "The user asks for the sound to be muted or unmuted.", Some("The user wants the sound muted, not unmuted.")),
    ("set_brightness", "The user asks for the screen brightness to be set or changed.", None),
    ("wifi", "The user asks for Wi-Fi to be turned on or off.", Some("The user wants Wi-Fi turned on, not off.")),
    ("focus", "The user asks for Focus (do not disturb) to be turned on or off.", Some("The user wants Focus turned on, not off.")),
    ("night_light", "The user asks for Night light to be turned on or off.", Some("The user wants Night light turned on, not off.")),
    ("lock", "The user asks for the screen to be locked.", None),
    ("sleep", "The user asks for the laptop to be put to sleep.", None),
    ("open_app", "The user asks for an app to be opened.", None),
];

/// Percent levels a Jev score picks between; the score, 0 to 4, is interpolated to 0 to 100.
const JEV_LEVELS: [&str; 5] = ["0%, the lowest", "25%", "50%, the middle", "75%", "100%, the highest"];

/// Whether Jev can route the sentence alone. It answers typed questions and writes no text, so a sentence
/// that needs a task, a fact, a question split from actions or an app's name goes to Ollama instead.
pub fn jev_can(text: &str, candidates: &[String]) -> bool {
    !wants_task(text) && !wants_remember(text) && candidates.is_empty()
}

/// The sentence (with the conversation, if any) as Jev's state, and its questions: one noul for each action,
/// which way for the switches, a score for the levels, and whether anything is asked beyond the actions.
pub fn jev_questions(text: &str, conversation: &str) -> (String, Value) {
    let state = if conversation.is_empty() {
        format!("A sentence typed into a desktop launcher:\n{text}")
    } else {
        format!("A conversation with a desktop assistant:\n{conversation}\n\nThe new message, typed into the launcher:\n{text}")
    };
    let mut questions = serde_json::Map::new();
    for (name, asks, way) in JEV_ACTIONS {
        questions.insert(format!("do_{name}"), json!({ "type": "noul", "instructions": asks }));
        if let Some(way) = way {
            questions.insert(format!("on_{name}"), json!({ "type": "noul", "instructions": way }));
        }
    }
    for (name, what) in [("set_volume", "volume"), ("set_brightness", "brightness")] {
        questions.insert(format!("level_{name}"), json!({ "type": "score", "instructions": format!("What {what} level does the user ask for?"), "criteria": JEV_LEVELS }));
    }
    questions.insert("question".to_owned(), json!({ "type": "noul", "instructions": "The user asks for something to be answered or explained, beyond requesting a change." }));
    (state, Value::Object(questions))
}

/// Jev's answers as a route: an action is chosen when its noul is 0.5 or more, with that as its confidence
/// (less when the switch's direction is unsure). `None` when Jev can't route it alone: an app's name is
/// needed, or a question comes with actions.
pub fn jev_parse(answers: &Value, text: &str) -> Result<Option<Routed>, String> {
    let noul = |id: &str| answers[id]["noul"].as_f64().map(|p| p.clamp(0.0, 1.0)).ok_or_else(|| format!("Jev's answer has no noul for “{id}”"));
    let level = |id: &str| answers[id]["score"].as_f64().filter(|score| score.is_finite()).map(|score| (score / 4.0 * 100.0).round().clamp(0.0, 100.0) as u8).ok_or_else(|| format!("Jev's answer has no score for “{id}”"));
    let mut actions = Vec::new();
    for (name, _, way) in JEV_ACTIONS {
        let asked = noul(&format!("do_{name}"))?;
        if asked < 0.5 {
            continue;
        }
        let mut sure = asked;
        let mut on = false;
        if way.is_some() {
            let p = noul(&format!("on_{name}"))?;
            on = p >= 0.5;
            sure = sure.min(p.max(1.0 - p));
        }
        let action = match name {
            "set_volume" => Action::SetVolume { percent: level("level_set_volume")? },
            "mute" => Action::Mute { on },
            "set_brightness" => Action::SetBrightness { percent: level("level_set_brightness")? },
            "wifi" => Action::Wifi { on },
            "focus" => Action::Focus { on },
            "night_light" => Action::NightLight { on },
            "lock" => Action::Lock,
            "sleep" => Action::Sleep,
            _ => return Ok(None),
        };
        actions.push(Chosen { action, confidence: sure });
    }
    let asks = noul("question")? >= 0.5;
    if asks && !actions.is_empty() {
        return Ok(None);
    }
    let confidence = actions.iter().map(|chosen| chosen.confidence).fold(1.0, f64::min);
    // As with Ollama, a sentence that asks for nothing is a question for Kith.
    let question = if actions.is_empty() { text.trim().to_owned() } else { String::new() };
    Ok(Some(Routed { actions, question, files: Vec::new(), remember: String::new(), task: None, confidence, engine: "jev" }))
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

    fn noul(p: f64) -> Value {
        json!({ "type": "noul", "noul": p })
    }

    fn jev_answers() -> Value {
        let mut answers = serde_json::Map::new();
        for (name, _, way) in JEV_ACTIONS {
            answers.insert(format!("do_{name}"), noul(0.01));
            if way.is_some() {
                answers.insert(format!("on_{name}"), noul(0.5));
            }
        }
        answers.insert("level_set_volume".to_owned(), json!({ "type": "score", "score": 1.0 }));
        answers.insert("level_set_brightness".to_owned(), json!({ "type": "score", "score": 2.0 }));
        answers.insert("question".to_owned(), noul(0.02));
        Value::Object(answers)
    }

    #[test]
    fn jev_picks_actions_and_their_arguments() {
        let mut answers = jev_answers();
        answers["do_mute"] = noul(0.97);
        answers["on_mute"] = noul(0.99);
        answers["do_set_volume"] = noul(0.9);
        answers["do_wifi"] = noul(0.8);
        answers["on_wifi"] = noul(0.1);
        let routed = jev_parse(&answers, "mute, volume to 25 and wifi off").unwrap().unwrap();
        assert_eq!(routed.engine, "jev");
        assert_eq!(
            routed.actions,
            vec![
                Chosen { action: Action::SetVolume { percent: 25 }, confidence: 0.9 },
                Chosen { action: Action::Mute { on: true }, confidence: 0.97 },
                Chosen { action: Action::Wifi { on: false }, confidence: 0.8 },
            ]
        );
        assert_eq!(routed.confidence, 0.8);
        assert!(routed.question.is_empty());
    }

    #[test]
    fn jev_hands_over_what_it_cannot_write() {
        let mut open = jev_answers();
        open["do_open_app"] = noul(0.9);
        assert_eq!(jev_parse(&open, "open files").unwrap(), None);
        let mut both = jev_answers();
        both["do_lock"] = noul(0.9);
        both["question"] = noul(0.9);
        assert_eq!(jev_parse(&both, "lock and tell me why").unwrap(), None);
        assert!(jev_can("mute", &[]) && !jev_can("every day at 8 mute", &[]) && !jev_can("mute", &["~/a".to_owned()]));
        let mut asked = jev_answers();
        asked["question"] = noul(0.9);
        let routed = jev_parse(&asked, " why is the sky blue ").unwrap().unwrap();
        assert!(routed.actions.is_empty());
        assert_eq!(routed.question, "why is the sky blue");
        assert!(jev_parse(&json!({}), "x").is_err());
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
