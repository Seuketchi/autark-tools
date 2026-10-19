//! Jev, TypeSafe AI's model, which answers typed questions about a state instead of writing text
//! (https://docs.typesafe.ai). One `POST /v1/systemone` carries a state and its questions; each comes
//! back as a choice, a score or a noul (a 0 to 1 yes/no) with a calibrated probability.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value, json};
use zbus::{OwnedObjectPath, OwnedVariant as OwnedValue, Variant};

const URL: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-latest";
const SERVICE: &str = "org.freedesktop.secrets";

/// Jev's API key: the keyring's, where Settings files it, else `TYPESAFE_API_KEY`. Never logged.
pub async fn key(connection: &zbus::Connection) -> Result<String, String> {
    if let Some(key) = std::env::var("TYPESAFE_API_KEY").ok().filter(|key| !key.is_empty()) {
        return Ok(key);
    }
    stored(connection, "jev").await.map_err(|err| format!("Jev's key: {err}"))
}

/// A key filed in the keyring under `application = autark-ai` and `key = <name>`.
pub async fn stored(connection: &zbus::Connection, name: &str) -> zbus::Result<String> {
    let service = zbus::Proxy::new(connection, SERVICE, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let attributes = HashMap::from([("application", "autark-ai"), ("key", name)]);
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service.call("SearchItems", &(attributes,)).await?;
    let Some(item) = unlocked.first() else {
        let why = if locked.is_empty() { "none saved: add it in Settings › AI" } else { "the keyring is locked" };
        return Err(zbus::Error::Failure(why.to_owned()));
    };
    // The "plain" algorithm: the secret crosses the session bus, which only this user's processes reach.
    let (_, session): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Variant::from(""))).await?;
    let item = zbus::Proxy::new(connection, SERVICE, item.as_str(), "org.freedesktop.Secret.Item").await?;
    let secret: zbus::Result<(OwnedObjectPath, Vec<u8>, Vec<u8>, String)> = item.call("GetSecret", &(&session,)).await;
    if let Ok(session) = zbus::Proxy::new(connection, SERVICE, session.as_str(), "org.freedesktop.Secret.Session").await {
        let _ = session.call_noreply("Close", &()).await;
    }
    String::from_utf8(secret?.2).map_err(|_| zbus::Error::Failure("the saved key isn't text".to_owned()))
}

/// Asks `questions` (id to typed question) about `state`, and returns the `answers` object.
pub fn ask(key: &str, state: &str, questions: Value) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).http_status_as_error(false).build().into();
    let body = json!({ "model": MODEL, "state": state, "questions": questions });
    let response = agent
        .post(URL)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|err| format!("Jev: {err}"))?;
    let status = response.status();
    let text = response.into_body().read_to_string().map_err(|err| format!("Jev's reply: {err}"))?;
    if !status.is_success() {
        let detail: String = text.chars().take(200).collect();
        return Err(format!("Jev: {status} {detail}"));
    }
    let mut reply: Value = serde_json::from_str(&text).map_err(|err| format!("Jev's reply: {err}"))?;
    Ok(reply["answers"].take())
}

/// A yes/no question's answer, 0 to 1.
pub fn noul(answers: &Value, id: &str) -> Result<f64, String> {
    answers[id]["noul"].as_f64().ok_or_else(|| format!("Jev's answer has no noul for “{id}”"))
}
