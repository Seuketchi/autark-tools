//! Ollama's chat API: answers shaped by a JSON schema, or text streamed as it's written.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config;

pub struct Ollama {
    agent: ureq::Agent,
    url: String,
    model: String,
    /// What `stream` writes with; `model` when none is set.
    answer_model: String,
}

impl Ollama {
    pub fn new(config: &config::Ollama) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(config.timeout_seconds)))
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent, url: config.url.trim_end_matches('/').to_owned(), model: config.model.clone(), answer_model: Some(config.answer_model.trim()).filter(|model| !model.is_empty()).unwrap_or(&config.model).to_owned() }
    }

    /// One answer that matches `schema`, at temperature 0 so the same input decides the same way. At most 400 tokens: a
    /// small model can run on inside a JSON string, and the caller would wait out the whole timeout.
    pub fn json(&self, system: &str, user: &str, schema: &Value) -> Result<Value, String> {
        let body = self.request(&self.model, system, user, json!({ "stream": false, "format": schema, "options": { "temperature": 0, "num_predict": 400 } }))?;
        let reply: Value = serde_json::from_reader(body).map_err(|err| format!("Ollama's reply: {err}"))?;
        let content = reply["message"]["content"].as_str().ok_or("Ollama's reply has no message")?;
        serde_json::from_str(content).map_err(|err| format!("the model's answer isn't the asked-for JSON: {err}"))
    }

    /// Generated text, handed to `on_piece` as it arrives, until it returns false. A low temperature: a small model
    /// at the default one drifts into inventing things.
    pub fn stream(&self, system: &str, user: &str, mut on_piece: impl FnMut(&str) -> bool) -> Result<(), String> {
        let body = self.request(&self.answer_model, system, user, json!({ "stream": true, "options": { "temperature": 0.3 } }))?;
        for line in BufReader::new(body).lines() {
            let line = line.map_err(|err| format!("reading from Ollama: {err}"))?;
            if line.is_empty() {
                continue;
            }
            let chunk: Value = serde_json::from_str(&line).map_err(|err| format!("Ollama's reply: {err}"))?;
            if let Some(error) = chunk["error"].as_str() {
                return Err(format!("Ollama: {error}"));
            }
            if let Some(piece) = chunk["message"]["content"].as_str().filter(|piece| !piece.is_empty())
                && !on_piece(piece)
            {
                break;
            }
            if chunk["done"].as_bool() == Some(true) {
                break;
            }
        }
        Ok(())
    }

    /// The models pulled into Ollama, by name.
    pub fn models(&self) -> Result<Vec<String>, String> {
        let response = self.agent.get(format!("{}/api/tags", self.url)).call().map_err(|err| format!("Ollama at {}: {err}", self.url))?;
        if !response.status().is_success() {
            return Err(format!("Ollama: {}", response.status()));
        }
        let reply: Value = serde_json::from_reader(response.into_body().into_reader()).map_err(|err| format!("Ollama's reply: {err}"))?;
        let mut names: Vec<String> = reply["models"].as_array().into_iter().flatten().filter_map(|model| model["name"].as_str().map(str::to_owned)).collect();
        names.sort();
        Ok(names)
    }

    fn request(&self, model: &str, system: &str, user: &str, extra: Value) -> Result<impl std::io::Read, String> {
        if model.is_empty() {
            return Err("no Ollama model chosen: set `model` under [ollama] in ai.toml".to_owned());
        }
        let mut body = json!({
            "model": model,
            "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
            // Loading the model from disk takes tens of seconds; keep it in memory between the calls of a work session.
            "keep_alive": "30m",
        });
        if let (Some(body), Value::Object(extra)) = (body.as_object_mut(), extra) {
            body.extend(extra);
        }
        let response = self
            .agent
            .post(format!("{}/api/chat", self.url))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|err| format!("Ollama at {}: {err}", self.url))?;
        let status = response.status();
        let mut body = response.into_body();
        if !status.is_success() {
            let text = body.read_to_string().unwrap_or_default();
            let error = serde_json::from_str::<Value>(&text).ok().and_then(|reply| reply["error"].as_str().map(str::to_owned));
            return Err(format!("Ollama: {}", error.unwrap_or_else(|| status.to_string())));
        }
        Ok(body.into_reader())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(model: &str, answer_model: &str) -> Ollama {
        Ollama::new(&config::Ollama { model: model.to_owned(), answer_model: answer_model.to_owned(), ..config::Ollama::default() })
    }

    #[test]
    fn answers_are_written_by_the_answer_model_when_one_is_set() {
        let ollama = with("gemma3:4b", "gemma3:12b");
        assert_eq!((ollama.model.as_str(), ollama.answer_model.as_str()), ("gemma3:4b", "gemma3:12b"));
    }

    #[test]
    fn without_one_the_model_writes_them_too() {
        for unset in ["", "  "] {
            assert_eq!(with("gemma3:4b", unset).answer_model, "gemma3:4b");
        }
    }
}
