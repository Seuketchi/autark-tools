//! `Route`: a sentence typed into the launcher becomes the laptop actions it asks for, each with how sure the model
//! is, and the question it asks. "Dim the screen and mute, then tell me why my last build stopped" is two actions
//! and a question. The model only picks actions from the catalog below; code checks every argument, and `Act` runs
//! the actions you choose.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Debug, PartialEq)]
pub struct Routed {
    pub actions: Vec<Chosen>,
    /// What's asked, put as a question on its own; empty when the sentence only asks for actions.
    pub question: String,
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
    /// Opens a web address in the default browser.
    OpenUrl {
        url: String,
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

const CATALOG: [(&str, &str); 10] = [
    ("set_volume", "set the sound volume (number: percent)"),
    ("mute", "mute or unmute the sound (on: true mutes)"),
    ("set_brightness", "set the screen brightness (number: percent)"),
    ("wifi", "turn Wi-Fi on or off"),
    ("focus", "turn Focus (do not disturb) on or off"),
    ("night_light", "turn Night light on or off"),
    ("lock", "lock the screen"),
    ("sleep", "put the laptop to sleep"),
    ("open_app", "open an app (text: its name)"),
    ("open_url", "open a website in the browser (text: its address, such as youtube.com)"),
];

/// The instructions.
pub fn system_prompt() -> String {
    let actions: Vec<String> = CATALOG.iter().map(|(name, what)| format!("- {name}: {what}")).collect();
    let mut prompt = String::from(
        "You turn one sentence typed into a desktop launcher into JSON. A sentence can ask for actions, ask a question, or both.\n\
         actions: each change the sentence asks for that an action below can make, with its argument and your confidence from 0 to 1 \
         that it was asked for. None if it asks for no change.\n\
         question: the part that asks something to be answered or explained, put as a question on its own. Empty if there's none.\n",
    );
    prompt.push_str("confidence: how sure you are of the whole answer, from 0 to 1.\n");
    prompt.push_str(&format!("Actions:\n{}\n", actions.join("\n")));
    prompt.push_str(
        "Examples (only actions the sentence asks for; none otherwise):\n\
         mute -> {\"actions\":[{\"name\":\"mute\",\"on\":true,\"confidence\":0.95}],\"question\":\"\",\"confidence\":0.95}\n\
         turn the volume down a bit and open files -> {\"actions\":[{\"name\":\"set_volume\",\"number\":30,\"confidence\":0.7},{\"name\":\"open_app\",\"text\":\"Files\",\"confidence\":0.95}],\"question\":\"\",\"confidence\":0.85}\n\
         open youtube -> {\"actions\":[{\"name\":\"open_url\",\"text\":\"youtube.com\",\"confidence\":0.9}],\"question\":\"\",\"confidence\":0.9}\n\
         why is the sky blue -> {\"actions\":[],\"question\":\"Why is the sky blue?\",\"confidence\":0.9}",
    );
    prompt
}

/// The answer's shape.
pub fn schema() -> Value {
    let names: Vec<&str> = CATALOG.iter().map(|(name, ..)| *name).collect();
    let properties = json!({
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
    json!({ "type": "object", "properties": properties, "required": ["actions", "question", "confidence"] })
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    actions: Vec<RawAction>,
    #[serde(default)]
    question: String,
    confidence: f64,
}

#[derive(Deserialize)]
struct RawAction {
    name: String,
    number: Option<f64>,
    on: Option<bool>,
    text: Option<String>,
    confidence: Option<f64>,
}

/// Whether the sentence names the website: its address (`youtube.com`) or its name (`youtube`); an address with a
/// path or query has to be written out whole. Asked to "look up" something, a small model invents plausible pages,
/// so `open_url` keeps to the sites the user asked for.
fn named_in(text: &str, url: &str) -> bool {
    let text = text.to_lowercase();
    let url = url.trim().to_lowercase();
    let rest = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
    let (host, path) = match rest.find(['/', '?', '#']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    if !path.trim_matches('/').is_empty() {
        return text.contains(rest) || text.contains(&url);
    }
    let host = host.strip_prefix("www.").unwrap_or(host);
    let name = host.split('.').next().unwrap_or_default();
    text.contains(host) || (name.len() >= 3 && text.contains(name))
}

/// Checks the model's answer against the catalog. With no actions or question, the whole sentence is the question:
/// you asked Kith, so Kith answers.
pub fn parse(answer: Value, text: &str) -> Result<Routed, String> {
    let answer: Answer = serde_json::from_value(answer).map_err(|err| format!("the route doesn't fit the schema: {err}"))?;
    let confidence = answer.confidence.clamp(0.0, 1.0);
    let actions = answer
        .actions
        .into_iter()
        .map(|raw| {
            let sure = raw.confidence.unwrap_or(confidence).clamp(0.0, 1.0);
            action(raw).map(|action| Chosen { action, confidence: sure })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|chosen| match &chosen.action {
            Action::OpenUrl { url } => named_in(text, url),
            _ => true,
        })
        .collect::<Vec<_>>();
    let mut question = answer.question.trim().to_owned();
    // A small model sometimes carries on after the question mark with its own answer and filler; the field is only the question.
    if let Some(end) = question.find('?') {
        question.truncate(end + 1);
    }
    if actions.is_empty() && question.is_empty() {
        question = text.trim().to_owned();
    }
    Ok(Routed { actions, question, confidence, engine: "ollama" })
}

/// What Jev is asked about each catalog action: whether the sentence asks for it, and for the ones with a
/// switch, which way (the noul is the chance of the first).
const JEV_ACTIONS: [(&str, &str, Option<&str>); 10] = [
    ("set_volume", "The user gives a sound volume level, or says to turn the sound up or down. Muting or unmuting alone is not this.", None),
    ("mute", "The user asks for the sound to be muted or unmuted.", Some("The user wants the sound muted, not unmuted.")),
    ("set_brightness", "The user asks for the screen brightness to be set or changed.", None),
    ("wifi", "The user asks for Wi-Fi to be turned on or off.", Some("The user wants Wi-Fi turned on, not off.")),
    ("focus", "The user asks for Focus (do not disturb) to be turned on or off.", Some("The user wants Focus turned on, not off.")),
    ("night_light", "The user asks for Night light to be turned on or off.", Some("The user wants Night light turned on, not off.")),
    ("lock", "The user asks for the screen to be locked.", None),
    ("sleep", "The user asks for the laptop to be put to sleep.", None),
    ("open_app", "The user asks for an app to be opened.", None),
    ("open_url", "The user asks for a website or a web page to be opened, such as YouTube or a web address.", None),
];

/// Percent levels a Jev score picks between; the score, 0 to 4, is interpolated to 0 to 100.
const JEV_LEVELS: [&str; 5] = ["0%, the lowest", "25%", "50%, the middle", "75%", "100%, the highest"];

/// The sentence as Jev's state, and its questions: one noul for each action, which way for the switches, a score
/// for the levels, and whether anything is asked beyond the actions.
pub fn jev_questions(text: &str) -> (String, Value) {
    let state = format!("A sentence typed into a desktop launcher:\n{text}");
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
    Ok(Some(Routed { actions, question, confidence, engine: "jev" }))
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
        "open_url" => Action::OpenUrl { url: text()? },
        _ => Action::OpenApp { name: text()? },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_and_a_question_from_one_sentence() {
        let answer = json!({ "confidence": 0.9, "question": "Why did my last build stop?", "actions": [
            { "name": "set_brightness", "number": 30, "confidence": 0.94 }, { "name": "mute", "on": true, "confidence": 0.97 } ] });
        let routed = parse(answer, "dim the screen and mute, then tell me why my last build stopped").unwrap();
        assert_eq!(routed.actions, vec![Chosen { action: Action::SetBrightness { percent: 30 }, confidence: 0.94 }, Chosen { action: Action::Mute { on: true }, confidence: 0.97 }]);
        assert_eq!(routed.question, "Why did my last build stop?");
        let json = serde_json::to_value(&routed).unwrap();
        assert_eq!(json["actions"][0], json!({ "action": "set_brightness", "percent": 30, "confidence": 0.94 }));
        assert_eq!(json["actions"][1], json!({ "action": "mute", "on": true, "confidence": 0.97 }));
        assert_eq!(json["engine"], "ollama");
    }

    #[test]
    fn arguments_are_checked() {
        let clamp = json!({ "confidence": 2.0, "question": "", "actions": [{ "name": "set_volume", "number": 140, "confidence": 0.8 }, { "name": "lock", "confidence": 0.5 }] });
        let routed = parse(clamp, "").unwrap();
        assert_eq!(routed.confidence, 1.0);
        assert_eq!(routed.actions[0].action, Action::SetVolume { percent: 100 });
        assert_eq!(serde_json::to_value(&routed.actions[1]).unwrap(), json!({ "action": "lock", "confidence": 0.5 }));
        assert!(parse(json!({ "confidence": 1, "actions": [{ "name": "wifi", "confidence": 1 }] }), "").is_err());
        assert!(parse(json!({ "confidence": 1, "actions": [{ "name": "rm_rf", "confidence": 1 }] }), "").is_err());
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
    fn a_website_is_an_action_with_its_address() {
        let answer = json!({ "confidence": 0.9, "question": "", "files": [], "actions": [ { "name": "open_url", "text": "youtube.com", "confidence": 0.9 } ] });
        let routed = parse(answer, "open youtube").unwrap();
        assert_eq!(routed.actions, vec![Chosen { action: Action::OpenUrl { url: "youtube.com".to_owned() }, confidence: 0.9 }]);
        assert_eq!(serde_json::to_value(&routed).unwrap()["actions"][0], json!({ "action": "open_url", "url": "youtube.com", "confidence": 0.9 }));
    }

    #[test]
    fn a_website_must_be_one_the_sentence_names() {
        assert!(named_in("open youtube", "youtube.com"));
        assert!(named_in("go to github.com", "https://github.com/"));
        assert!(named_in("open https://example.org/a/b please", "https://example.org/a/b"));
        assert!(!named_in("lookup the latest news on the Linux 7.3 release", "https://www.phoronix.com/news/Linux-7.3-Release-Notes.html"));
        assert!(!named_in("open the browser", "https://google.com"));
        assert!(!named_in("open youtube", "https://youtube.com/watch?v=abc"));
        let invented = json!({ "confidence": 0.9, "question": "", "files": [], "actions": [ { "name": "open_url", "text": "https://www.phoronix.com/news/x.html", "confidence": 0.9 } ] });
        let routed = parse(invented, "lookup the latest news on Linux 7.3").unwrap();
        assert!(routed.actions.is_empty());
        assert_eq!(routed.question, "lookup the latest news on Linux 7.3");
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
        let mut asked = jev_answers();
        asked["question"] = noul(0.9);
        let routed = jev_parse(&asked, " why is the sky blue ").unwrap().unwrap();
        assert!(routed.actions.is_empty());
        assert_eq!(routed.question, "why is the sky blue");
        assert!(jev_parse(&json!({}), "x").is_err());
    }

    #[test]
    fn nothing_routed_asks_the_whole_sentence() {
        let routed = parse(json!({ "confidence": 0.4, "question": " ", "actions": [] }), "how much swap do I have ").unwrap();
        assert_eq!(routed.question, "how much swap do I have");
        assert!(routed.actions.is_empty());
    }
}
