//! `org.autark.AI1` on the session bus.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;

use crate::config::Ai as Config;
use crate::ollama::Ollama;
use crate::{act, check, files, log, memory, route};

pub const PATH: &str = "/org/autark/AI1";

#[derive(zbus::DBusError, Debug)]
#[zbus(prefix = "org.autark.AI1.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// AI, or this feature, is switched off in settings.
    Disabled(String),
    /// The engine failed or gave an answer that doesn't fit.
    Engine(String),
}

pub struct Ai {
    /// Swapped whole when ai.toml changes; a call under way keeps the one it started with.
    config: RwLock<Arc<Config>>,
    ollama: RwLock<Arc<Ollama>>,
    next_answer: AtomicU32,
    /// Answers to stop at their next piece.
    stopped: Arc<Mutex<HashSet<u32>>>,
}

impl Ai {
    pub fn new(config: Config) -> Self {
        let ollama = Arc::new(Ollama::new(&config.ollama));
        Self { config: RwLock::new(Arc::new(config)), ollama: RwLock::new(ollama), next_answer: AtomicU32::new(1), stopped: Arc::default() }
    }

    /// Takes a changed ai.toml.
    pub fn reload(&self, config: Config) {
        let ollama = Arc::new(Ollama::new(&config.ollama));
        if let (Ok(mut old), Ok(mut old_ollama)) = (self.config.write(), self.ollama.write()) {
            *old = Arc::new(config);
            *old_ollama = ollama;
        }
    }

    fn config(&self) -> Arc<Config> {
        self.config.read().map(|config| config.clone()).unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn ollama(&self) -> Arc<Ollama> {
        self.ollama.read().map(|ollama| ollama.clone()).unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn allowed(&self, feature: bool, name: &str) -> Result<(), Error> {
        if !self.config().enabled {
            return Err(Error::Disabled("AI is off: set `enabled = true` in ai.toml".to_owned()));
        }
        if !feature {
            return Err(Error::Disabled(format!("{name} is off in ai.toml")));
        }
        Ok(())
    }

    /// Runs a blocking Ollama call off the bus thread, and logs it.
    async fn json(&self, call: &'static str, input: Value, system: String, user: String, schema: Value) -> Result<Value, String> {
        let ollama = self.ollama();
        let start = Instant::now();
        let answer = blocking::unblock(move || ollama.json(&system, &user, &schema)).await;
        log::record(call, "ollama", input, &answer, start.elapsed());
        answer
    }

    /// Streams an answer on a thread of its own as `AnswerPiece` signals, then `AnswerDone`, and returns its id.
    /// `logged` is added to the call log's input.
    fn answer(&self, emitter: SignalEmitter<'_>, prompt: String, context: String, logged: Value) -> u32 {
        let id = self.next_answer.fetch_add(1, Ordering::Relaxed);
        let emitter = emitter.into_owned();
        let ollama = self.ollama();
        let stopped = self.stopped.clone();
        std::thread::spawn(move || {
            let system = "You are Kith, the assistant built into the Autark desktop. Answer briefly and plainly. \
                          Use the context when it's given; say so when it doesn't hold the answer.";
            let user = if context.is_empty() { prompt.clone() } else { format!("Context:\n{context}\n\nQuestion: {prompt}") };
            let start = Instant::now();
            let mut text = String::new();
            let result = ollama.stream(system, &user, |piece| {
                if stopped.lock().map(|mut stopped| stopped.remove(&id)).unwrap_or(false) {
                    return false;
                }
                text.push_str(piece);
                if let Err(err) = async_io::block_on(Ai::answer_piece(&emitter, id, piece)) {
                    eprintln!("ai: sending an answer: {err}");
                }
                true
            });
            let answer = result.map(|()| json!(text));
            let mut input = json!({ "prompt": prompt, "context": context });
            if let (Some(input), Value::Object(logged)) = (input.as_object_mut(), logged) {
                input.extend(logged);
            }
            log::record("ask", "ollama", input, &answer, start.elapsed());
            let error = answer.err().unwrap_or_default();
            if let Ok(mut stopped) = stopped.lock() {
                stopped.remove(&id);
            }
            if let Err(err) = async_io::block_on(Ai::answer_done(&emitter, id, &error)) {
                eprintln!("ai: sending an answer: {err}");
            }
        });
        id
    }
}

#[zbus::interface(name = "org.autark.AI1")]
impl Ai {
    /// Turns a sentence into the actions it asks for and the question it asks, as JSON:
    /// `{"actions":[{"action":"mute","on":true,"confidence":0.97}],"question":"Why did my build stop?",
    /// "files":["~/autark/log.md"],"confidence":0.9,"engine":"ollama"}`.
    async fn route(&self, text: String) -> Result<String, Error> {
        self.allowed(self.config().features.route, "route")?;
        let facts = memory::facts();
        let candidates = route::candidates(&text, &facts);
        let user = route::user(&text, &candidates);
        let answer = self.json("route", json!(text), route::system_prompt(&facts), user, route::schema(&candidates)).await;
        let routed = answer.and_then(|answer| route::parse(answer, &text, &candidates)).map_err(Error::Engine)?;
        serde_json::to_string(&routed).map_err(|err| Error::Engine(err.to_string()))
    }

    /// How relevant each item is to the query, 0 to 1, in the items' order.
    async fn rank(&self, query: String, items: Vec<String>) -> Result<Vec<f64>, Error> {
        self.allowed(self.config().features.rank, "rank")?;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let listed: Vec<String> = items.iter().enumerate().map(|(n, item)| format!("{}. {item}", n + 1)).collect();
        let user = format!("Query: {query}\nItems:\n{}", listed.join("\n"));
        let system = "Score how relevant each numbered item is to the query, from 0 (unrelated) to 1 (exactly what's meant). \
                      Give one score per item, in order.";
        let schema = json!({
            "type": "object",
            "properties": { "scores": { "type": "array", "items": { "type": "number" }, "minItems": items.len(), "maxItems": items.len() } },
            "required": ["scores"],
        });
        let answer = self.json("rank", json!({ "query": query, "items": items }), system.to_owned(), user, schema).await.map_err(Error::Engine)?;
        let scores: Vec<f64> = answer["scores"].as_array().into_iter().flatten().filter_map(Value::as_f64).map(|s| s.clamp(0.0, 1.0)).collect();
        if scores.len() != items.len() {
            return Err(Error::Engine(format!("got {} scores for {} items", scores.len(), items.len())));
        }
        Ok(scores)
    }

    /// The probability that running `command` does harm, with a short reason. Destructive commands
    /// score 1 by rule, even with AI off.
    async fn check(&self, command: String) -> Result<(f64, String), Error> {
        if let Some(reason) = check::rules(&command) {
            log::record("check", "rules", json!(command), &Ok(json!({ "risk": 1.0, "reason": reason })), Default::default());
            return Ok((1.0, reason.to_owned()));
        }
        if self.allowed(self.config().features.check, "check").is_err() {
            return Ok((0.0, String::new()));
        }
        let answer = self.json("check", json!(command), check::SYSTEM_PROMPT.to_owned(), command.clone(), check::schema()).await.map_err(Error::Engine)?;
        let risk = answer["risk"].as_f64().ok_or_else(|| Error::Engine("no risk in the answer".to_owned()))?;
        Ok((risk.clamp(0.0, 1.0), answer["reason"].as_str().unwrap_or_default().to_owned()))
    }

    /// Starts a generated answer and returns its id. The text arrives as `AnswerPiece` signals,
    /// then `AnswerDone`, whose error is empty on success.
    async fn ask(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, prompt: String, context: String) -> Result<u32, Error> {
        self.allowed(self.config().features.ask, "ask")?;
        Ok(self.answer(emitter, prompt, context, json!({})))
    }

    /// `Ask` with files for context, as `Route` picked them: reads the ones Kith may read (see `files`) and
    /// says how each went, an empty reason for read, before the answer streams.
    async fn ask_files(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, prompt: String, files: Vec<String>) -> Result<(u32, Vec<(String, String)>), Error> {
        self.allowed(self.config().features.ask, "ask")?;
        let (context, outcome) = blocking::unblock(move || files::read(&files)).await;
        let read = json!(outcome.iter().map(|(path, why)| if why.is_empty() { json!(path) } else { json!({ "path": path, "not_read": why }) }).collect::<Vec<_>>());
        Ok((self.answer(emitter, prompt, context, json!({ "files": read })), outcome))
    }

    /// Stops an answer still being written; its `AnswerDone` follows, with no error.
    async fn stop(&self, id: u32) {
        if let Ok(mut stopped) = self.stopped.lock() {
            stopped.insert(id);
        }
    }

    /// Runs actions as `Route` gives them (a JSON array of `{"action": …}`), once you've chosen them, and
    /// says how each went: empty for done, else why not. Code runs them, with AI on or off.
    async fn act(&self, #[zbus(connection)] connection: &zbus::Connection, actions: String) -> Result<Vec<String>, Error> {
        let parsed: Vec<route::Action> = serde_json::from_str(&actions).map_err(|err| Error::Engine(format!("not actions from the catalog: {err}")))?;
        let start = Instant::now();
        let results = act::run(connection, &parsed).await;
        log::record("act", "code", serde_json::from_str(&actions).unwrap_or(Value::Null), &Ok(json!(results)), start.elapsed());
        Ok(results)
    }

    /// The models pulled into Ollama, for Settings to choose from. Asks nothing of a model, so it answers
    /// with AI off too.
    async fn models(&self) -> Result<Vec<String>, Error> {
        let ollama = self.ollama();
        blocking::unblock(move || ollama.models()).await.map_err(Error::Engine)
    }

    #[zbus(signal)]
    async fn answer_piece(emitter: &SignalEmitter<'_>, id: u32, text: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn answer_done(emitter: &SignalEmitter<'_>, id: u32, error: &str) -> zbus::Result<()>;
}
