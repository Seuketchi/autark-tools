//! `Route`: a sentence typed into the launcher becomes actions with arguments, a question or a
//! search. The model only picks from the catalog below; code checks every argument, and the
//! launcher runs the actions.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Debug, PartialEq)]
pub struct Routed {
    #[serde(flatten)]
    pub route: Route,
    /// How sure the engine is, 0 to 1. Below the launcher's threshold it shows options instead of
    /// acting.
    pub confidence: f64,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Route {
    Actions { actions: Vec<Action> },
    Question { text: String },
    Search { query: String },
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    SetVolume { percent: u8 },
    Mute { on: bool },
    SetBrightness { percent: u8 },
    Wifi { on: bool },
    Focus { on: bool },
    NightLight { on: bool },
    Lock,
    Sleep,
    OpenApp { name: String },
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

pub fn system_prompt() -> String {
    let actions: Vec<String> = CATALOG.iter().map(|(name, what)| format!("- {name}: {what}")).collect();
    format!(
        "You turn one sentence typed into a desktop launcher into JSON.\n\
         kind \"actions\": the sentence asks to change something the actions below can do; list each one with its argument.\n\
         kind \"question\": it asks something to be answered or explained.\n\
         kind \"search\": it names a file, app or setting to find; put the words to search for in query.\n\
         confidence: how sure you are, from 0 to 1.\n\
         Actions:\n{}",
        actions.join("\n")
    )
}

pub fn schema() -> Value {
    let names: Vec<&str> = CATALOG.iter().map(|(name, ..)| *name).collect();
    json!({
        "type": "object",
        "properties": {
            "kind": { "enum": ["actions", "question", "search"] },
            "actions": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "name": { "enum": names },
                    "number": { "type": "number" },
                    "on": { "type": "boolean" },
                    "text": { "type": "string" },
                },
                "required": ["name"],
            }},
            "query": { "type": "string" },
            "confidence": { "type": "number" },
        },
        "required": ["kind", "confidence"],
    })
}

#[derive(Deserialize)]
struct Answer {
    kind: String,
    #[serde(default)]
    actions: Vec<RawAction>,
    #[serde(default)]
    query: String,
    confidence: f64,
}

#[derive(Deserialize)]
struct RawAction {
    name: String,
    number: Option<f64>,
    on: Option<bool>,
    text: Option<String>,
}

/// Checks the model's answer against the catalog. `text` is the sentence, used as the question
/// or search when the model leaves those out.
pub fn parse(answer: Value, text: &str) -> Result<Routed, String> {
    let answer: Answer = serde_json::from_value(answer).map_err(|err| format!("the route doesn't fit the schema: {err}"))?;
    let confidence = answer.confidence.clamp(0.0, 1.0);
    let or_text = |words: String| if words.trim().is_empty() { text.to_owned() } else { words };
    let route = match answer.kind.as_str() {
        "actions" => {
            let actions = answer.actions.into_iter().map(action).collect::<Result<Vec<_>, _>>()?;
            if actions.is_empty() {
                return Err("the model chose actions but named none".to_owned());
            }
            Route::Actions { actions }
        }
        "question" => Route::Question { text: text.to_owned() },
        "search" => Route::Search { query: or_text(answer.query) },
        other => return Err(format!("unknown route kind {other:?}")),
    };
    Ok(Routed { route, confidence })
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
    fn dim_and_mute() {
        let answer = json!({ "kind": "actions", "confidence": 0.93, "actions": [
            { "name": "set_brightness", "number": 30 }, { "name": "mute", "on": true } ] });
        let routed = parse(answer, "dim the screen and mute").unwrap();
        assert_eq!(routed.route, Route::Actions { actions: vec![Action::SetBrightness { percent: 30 }, Action::Mute { on: true }] });
        let json = serde_json::to_value(&routed).unwrap();
        assert_eq!(json["kind"], "actions");
        assert_eq!(json["actions"][0], json!({ "action": "set_brightness", "percent": 30 }));
    }

    #[test]
    fn arguments_are_checked() {
        let clamp = json!({ "kind": "actions", "confidence": 2.0, "actions": [{ "name": "set_volume", "number": 140 }] });
        let routed = parse(clamp, "").unwrap();
        assert_eq!(routed.confidence, 1.0);
        assert_eq!(routed.route, Route::Actions { actions: vec![Action::SetVolume { percent: 100 }] });
        assert!(parse(json!({ "kind": "actions", "confidence": 1, "actions": [{ "name": "wifi" }] }), "").is_err());
        assert!(parse(json!({ "kind": "actions", "confidence": 1, "actions": [{ "name": "rm_rf" }] }), "").is_err());
    }

    #[test]
    fn search_falls_back_to_the_sentence() {
        let routed = parse(json!({ "kind": "search", "confidence": 0.7 }), "tax receipts").unwrap();
        assert_eq!(routed.route, Route::Search { query: "tax receipts".to_owned() });
    }
}
