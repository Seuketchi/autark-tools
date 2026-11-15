//! `org.autark.AI1` on the session bus.

use std::collections::HashSet;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;

use crate::config::Ai as Config;
use crate::ollama::Ollama;
use crate::{act, check, events, jev, log, route, senses, suggest};

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
    suggestions: Mutex<suggest::Policy>,
    busy: Arc<Busy>,
}

/// What Kith is doing, for its face: thinking while a sentence is routed or an answer hasn't started, speaking
/// while one is written. Counted, since several can be under way.
#[derive(Default)]
struct Busy {
    thinking: AtomicI32,
    speaking: AtomicI32,
    said: Mutex<&'static str>,
}

impl Busy {
    fn state(&self) -> &'static str {
        if self.speaking.load(Ordering::SeqCst) > 0 {
            "speaking"
        } else if self.thinking.load(Ordering::SeqCst) > 0 {
            "thinking"
        } else {
            "idle"
        }
    }

    /// Adds to the counts, and returns the new state if it changed.
    fn change(&self, thinking: i32, speaking: i32) -> Option<&'static str> {
        self.thinking.fetch_add(thinking, Ordering::SeqCst);
        self.speaking.fetch_add(speaking, Ordering::SeqCst);
        let state = self.state();
        let mut said = self.said.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (*said != state).then(|| {
            *said = state;
            state
        })
    }
}

impl Ai {
    pub fn new(config: Config) -> Self {
        let ollama = Arc::new(Ollama::new(&config.ollama));
        Self {
            config: RwLock::new(Arc::new(config)),
            ollama: RwLock::new(ollama),
            next_answer: AtomicU32::new(1),
            stopped: Arc::default(),
            suggestions: Mutex::new(suggest::Policy::load()),
            busy: Arc::new(Busy { said: Mutex::new("idle"), ..Default::default() }),
        }
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

    /// The instructions for a generated answer: the rules, what Kith can do when `question` asks, and how it sounds.
    fn ask_system(&self, question: &str) -> String {
        let can = if asks_abilities(question) { ABILITIES } else { "" };
        format!("{ASK_SYSTEM}{can}{ASK_VOICE}")
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

    /// What Kith can see of the laptop that `question` is about, as context for the answer (see `senses`). Empty when
    /// the question is about none of it.
    async fn senses(&self, question: &str) -> String {
        let folder = self.config().suggestions.build_logs.clone();
        let subjects = senses::subjects(question);
        let seen = blocking::unblock(move || {
            subjects
                .into_iter()
                .map(|subject| match subject {
                    senses::Subject::System => senses::system(),
                    senses::Subject::Build => senses::build(&folder),
                })
                .collect::<Vec<String>>()
        })
        .await;
        seen.join("\n\n")
    }

    /// `Check`'s risk from Jev, logged like any call. The command leaves the laptop, which is why `[jev] terminal` is off by default.
    async fn jev_risk(&self, connection: &zbus::Connection, command: &str) -> Result<f64, String> {
        let start = Instant::now();
        let result = async {
            let key = jev::key(connection).await?;
            let (state, questions) = check::jev_question(command);
            let answers = blocking::unblock(move || jev::ask(&key, &state, questions)).await?;
            jev::noul(&answers, "risk")
        }
        .await;
        log::record("check", "jev", json!(command), &result.clone().map(|risk| json!({ "risk": risk })), start.elapsed());
        result.map(|risk| risk.clamp(0.0, 1.0))
    }

    /// `Route` from Jev, logged like any call. `None` when Jev can't route the sentence alone (see `route::jev_parse`).
    /// The sentence leaves the laptop, which is why `[jev] launcher` is off by default.
    async fn jev_route(&self, connection: &zbus::Connection, text: &str) -> Result<Option<route::Routed>, String> {
        let start = Instant::now();
        let result = async {
            let key = jev::key(connection).await?;
            let (state, questions) = route::jev_questions(text);
            let answers = blocking::unblock(move || jev::ask(&key, &state, questions)).await?;
            route::jev_parse(&answers, text)
        }
        .await;
        let logged = result.as_ref().map(|routed| json!({ "handed_over": routed.is_none(), "actions": routed.as_ref().map(|routed| routed.actions.len()) })).map_err(Clone::clone);
        log::record("route", "jev", json!(text), &logged, start.elapsed());
        result
    }

    /// How much a notification needs the user's attention now, from Jev, logged like any call. Its text leaves the
    /// laptop, which is why `[jev] notify` is off by default.
    async fn jev_urgent(&self, connection: &zbus::Connection, app: &str, summary: &str, body: &str) -> Result<f64, String> {
        let start = Instant::now();
        let result = async {
            let key = jev::key(connection).await?;
            let (state, questions) = check::jev_urgent(app, summary, body);
            let answers = blocking::unblock(move || jev::ask(&key, &state, questions)).await?;
            jev::noul(&answers, "urgent").map(|urgent| urgent.clamp(0.0, 1.0))
        }
        .await;
        log::record("urgent", "jev", json!({ "app": app, "summary": summary, "body": body }), &result.clone().map(|urgent| json!({ "urgent": urgent })), start.elapsed());
        result
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
    fn answer(&self, emitter: SignalEmitter<'_>, ask: Ask) -> u32 {
        let id = self.next_answer.fetch_add(1, Ordering::Relaxed);
        let emitter = emitter.into_owned();
        let ollama = self.ollama();
        let stopped = self.stopped.clone();
        let busy = self.busy.clone();
        std::thread::spawn(move || {
            let announce = |state: Option<&'static str>| {
                if let Some(state) = state
                    && let Err(err) = async_io::block_on(Ai::state_changed(&emitter, state))
                {
                    eprintln!("ai: sending Kith's state: {err}");
                }
            };
            announce(busy.change(1, 0));
            let mut started = false;
            let user = if ask.context.is_empty() { ask.prompt.clone() } else { format!("{}\n\nQuestion: {}", ask.context, ask.prompt) };
            let start = Instant::now();
            let mut text = String::new();
            let mut write = |piece: &str| -> bool {
                if stopped.lock().map(|mut stopped| stopped.remove(&id)).unwrap_or(false) {
                    return false;
                }
                if !started {
                    started = true;
                    announce(busy.change(-1, 1));
                }
                let offset = text.len() as u32;
                text.push_str(piece);
                if let Err(err) = async_io::block_on(Ai::answer_piece(&emitter, id, offset, piece)) {
                    eprintln!("ai: sending an answer: {err}");
                }
                true
            };
            let result = match &ask.verbatim {
                Some(said) => {
                    write(said);
                    Ok(())
                }
                None => ollama.stream(&ask.system, &user, &mut write),
            };
            let answer = result.map(|()| json!(text));
            log::record("ask", if ask.verbatim.is_some() { "code" } else { "ollama" }, json!({ "prompt": ask.prompt, "context": ask.context }), &answer, start.elapsed());
            announce(if started { busy.change(0, -1) } else { busy.change(-1, 0) });
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

    /// Routes `text`.
    async fn routed(&self, emitter: &SignalEmitter<'_>, connection: &zbus::Connection, text: &str) -> Result<route::Routed, Error> {
        self.allowed(self.config().features.route, "route")?;
        let config = self.config();
        if config.jev.launcher && !config.local_only {
            self.announce(emitter, self.busy.change(1, 0)).await;
            let routed = self.jev_route(connection, text).await;
            self.announce(emitter, self.busy.change(-1, 0)).await;
            // Ollama routes what Jev hands over or can't answer, so the launcher never loses its answer.
            match routed {
                Ok(Some(routed)) => return Ok(routed),
                Ok(None) => {}
                Err(err) => eprintln!("ai: {err}; Ollama routes it instead"),
            }
        }
        self.announce(emitter, self.busy.change(1, 0)).await;
        let answer = self.json("route", json!(text), route::system_prompt(), text.to_owned(), route::schema()).await;
        self.announce(emitter, self.busy.change(-1, 0)).await;
        answer.and_then(|answer| route::parse(answer, text)).map_err(Error::Engine)
    }

    async fn announce(&self, emitter: &SignalEmitter<'_>, state: Option<&'static str>) {
        if let Some(state) = state
            && let Err(err) = Ai::state_changed(emitter, state).await
        {
            eprintln!("ai: sending Kith's state: {err}");
        }
    }

    pub fn limits(&self) -> suggest::Limits {
        self.config().suggestions.clone()
    }

    /// The limits, while AI and suggestions are on.
    pub fn suggesting(&self) -> Option<suggest::Limits> {
        let config = self.config();
        (config.enabled && config.features.suggest).then(|| config.suggestions.clone())
    }

    fn policy(&self) -> std::sync::MutexGuard<'_, suggest::Policy> {
        self.suggestions.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Takes what one look found: the first event the policy lets through is rated, and shown if it's worth it,
    /// as a quiet notification and the `Suggestion` signal, and kept open until it's answered.
    pub async fn consider(&self, emitter: &SignalEmitter<'_>, connection: &zbus::Connection, found: Vec<events::Event>) {
        let Some(limits) = self.suggesting() else { return };
        let Some(event) = self.policy().pick(found, suggest::now(), Instant::now(), limits.per_hour) else { return };
        let focus = suggest::focus(connection).await;
        let input = json!({ "kind": event.kind, "facts": event.facts, "offer": event.offer.as_ref().map(|(_, label)| label), "focus": focus });
        let answer = match self.json("suggest", input, suggest::SYSTEM.to_owned(), suggest::user(&event, focus), suggest::schema()).await {
            Ok(answer) => answer,
            Err(err) => return eprintln!("ai: rating a suggestion: {err}"),
        };
        let worth = answer["worth"].as_f64().unwrap_or_default().clamp(0.0, 1.0);
        if worth < suggest::BAR {
            return;
        }
        let text = answer["text"].as_str().map(str::trim).filter(|text| !text.is_empty()).unwrap_or(&event.facts).to_owned();
        self.policy().shown(suggest::now());
        let notification = suggest::notify(connection, &text).await.unwrap_or_else(|err| {
            eprintln!("ai: sending a suggestion's notification: {err}");
            0
        });
        let (offer, offer_label) = match &event.offer {
            Some((action, label)) => (serde_json::to_value(action).ok(), label.clone()),
            None => (None, String::new()),
        };
        let suggestion = suggest::Open { id: 0, time: suggest::now(), event: event.kind, key: event.key(), label: event.label.clone(), text: text.clone(), offer, offer_label, worth, notification };
        let id = self.policy().open(suggestion);
        if let Err(err) = Ai::suggestion(emitter, id, &text).await {
            eprintln!("ai: sending a suggestion: {err}");
        }
        say_suggestions_changed(emitter).await;
    }
}

/// What an answer is asked from.
struct Ask {
    system: String,
    prompt: String,
    context: String,
    /// An answer written already, shown as it is with no model.
    verbatim: Option<String>,
}

const ASK_SYSTEM: &str = "You are Kith, the assistant built into the Autark desktop, which looks after this laptop. Answer the question that was asked, \
                          in one or two plain sentences, unless the user asks for a list or more detail: then give it, one item to a line, each starting with \"- \". \
                          You know only what is given with the question. When something asked for isn't given, say so in one sentence. Never invent events, \
                          names, numbers, links or news, and never claim an ability you haven't been told you have.";

/// How Kith sounds. It never names "Done.": the launcher marks an action done itself, and a small model repeats
/// whatever it is told not to say.
const ASK_VOICE: &str = "\nDon't say \"sir\" or \"ma'am\", and never end with a question or an offer: reply to a greeting with a greeting.";

/// What Kith can do, so the model neither denies an ability it has nor makes one up.
const ABILITIES: &str = "\nWhat Kith can do:\n\
                         - Propose actions for the user to tick: volume, brightness, mute, Wi-Fi, Focus, night light, lock, sleep, open an app or a website.\n\
                         - Look at this laptop: disk, battery, memory, uptime and how the last build went.\n\
                         - Suggest something when it notices an app running away, the battery low, a build ending or a new network.\n\
                         - Check pasted terminal commands for harm.\n\
                         It keeps no conversation and remembers nothing between questions.";

/// What Kith says when asked plainly what it can do.
const CAN_DO: &str = "I look after this laptop. I can propose actions for you to tick: volume, brightness, mute, Wi-Fi, Focus, night light, lock, sleep, \
                      and opening an app or a website. I can tell you about its disk, battery, memory and uptime and how the last build went, suggest \
                      something when an app runs away, the battery runs low, a build ends or a new network is in range, and check pasted terminal commands.";

/// Whether a question is about what Kith can do. The list of abilities goes in only then: a small model that has it
/// in front of it offers those things, or reports a battery level, to a plain "hi".
fn asks_abilities(question: &str) -> bool {
    let question = question.to_lowercase();
    ["can you", "could you", "able to", "what do you do", "what can", "capab", "abilit", "feature", "how about", "do you have", "help me"].iter().any(|ask| question.contains(ask))
}

/// Whether a question is plainly "what can you do". That has one answer, so code gives it (`CAN_DO`) instead of a model
/// that took 30 s to describe it wrongly.
fn asks_what_it_can_do(question: &str) -> bool {
    let question = question.to_lowercase();
    ["what can you do", "what do you do", "what are you able", "what are you capable", "your capabilities", "what can kith do"].iter().any(|ask| question.contains(ask))
}

/// Says the open suggestions changed, for the bar's dot and the centre's widget.
async fn say_suggestions_changed(emitter: &SignalEmitter<'_>) {
    if let Err(err) = Ai::suggestions_changed(emitter).await {
        eprintln!("ai: sending a change in suggestions: {err}");
    }
}

#[zbus::interface(name = "org.autark.AI1")]
impl Ai {
    /// Turns a sentence into the laptop actions it asks for and the question it asks, as JSON:
    /// `{"actions":[{"action":"mute","on":true,"confidence":0.97}],"question":"Why did my build stop?","confidence":0.9,"engine":"ollama"}`.
    async fn route(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, text: String) -> Result<String, Error> {
        let routed = self.routed(&emitter, connection, &text).await?;
        serde_json::to_string(&routed).map_err(|err| Error::Engine(err.to_string()))
    }

    /// Answers a suggestion: `accept` runs its offer through `Act` and returns how it went (empty for done, else
    /// why not); `not_now` passes; `never` also stops this kind and subject being suggested again.
    async fn answer_suggestion(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, entry: u64, answer: String) -> Result<String, Error> {
        if !["accept", "not_now", "never"].contains(&answer.as_str()) {
            return Err(Error::Engine(format!("“{answer}” isn't an answer: accept, not_now or never")));
        }
        let open = self.policy().answer(entry, suggest::now()).ok_or_else(|| Error::Engine("that suggestion has been answered already or has passed by".to_owned()))?;
        let result = match answer.as_str() {
            "accept" => match &open.offer {
                Some(offer) => match serde_json::from_value::<route::Action>(offer.clone()) {
                    Ok(action) => {
                        let start = Instant::now();
                        let ran = act::run(connection, &[action]).await;
                        log::record("act", "code", json!([offer]), &Ok(json!(ran)), start.elapsed());
                        ran.into_iter().next().unwrap_or_default()
                    }
                    Err(err) => format!("not an action: {err}"),
                },
                None => "that suggestion offers nothing".to_owned(),
            },
            "never" => {
                self.policy().never(&open.key, &open.label);
                String::new()
            }
            _ => String::new(),
        };
        say_suggestions_changed(&emitter).await;
        if open.notification != 0 {
            suggest::close(connection, open.notification).await;
        }
        Ok(result)
    }

    /// The suggestions still open and standing, oldest first, as a JSON array of
    /// `{"id","time","event","key","label","text","offer","offer_label","worth","notification"}`.
    async fn open_suggestions(&self) -> Result<String, Error> {
        serde_json::to_string(self.policy().standing(suggest::now())).map_err(|err| Error::Engine(err.to_string()))
    }

    /// What you said never to suggest again: each one's key and how to name it.
    async fn never_suggested(&self) -> Vec<(String, String)> {
        self.policy().nevers().iter().map(|never| (never.key.clone(), never.label.clone())).collect()
    }

    /// Lets something you said never to suggest be suggested again.
    async fn suggest_again(&self, key: String) {
        self.policy().again(&key);
    }

    /// How much a notification needs the user's attention now, 0 to 1, for Focus to let the urgent ones through.
    /// Jev only: it is off, an error, unless `[jev] notify` is on and `local_only` is not, so a notification's
    /// text never goes anywhere else.
    async fn urgent(&self, #[zbus(connection)] connection: &zbus::Connection, app: String, summary: String, body: String) -> Result<f64, Error> {
        let config = self.config();
        if !config.enabled {
            return Err(Error::Disabled("AI is off: set `enabled = true` in ai.toml".to_owned()));
        }
        if !config.jev.notify || config.local_only {
            return Err(Error::Disabled("urgent is off in ai.toml".to_owned()));
        }
        self.jev_urgent(connection, &app, &summary, &body).await.map_err(Error::Engine)
    }

    /// The probability that running `command` does harm, with a short reason. Destructive commands
    /// score 1 by rule, even with AI off.
    async fn check(&self, #[zbus(connection)] connection: &zbus::Connection, command: String) -> Result<(f64, String), Error> {
        if let Some(reason) = check::rules(&command) {
            log::record("check", "rules", json!(command), &Ok(json!({ "risk": 1.0, "reason": reason })), Default::default());
            return Ok((1.0, reason.to_owned()));
        }
        if self.allowed(self.config().features.check, "check").is_err() {
            return Ok((0.0, String::new()));
        }
        let config = self.config();
        if config.jev.terminal && !config.local_only {
            // Jev gives a risk but no reason; if it can't answer, Ollama does, so the check never goes missing.
            match self.jev_risk(connection, &command).await {
                Ok(risk) => return Ok((risk, String::new())),
                Err(err) => eprintln!("ai: {err}; Ollama checks it instead"),
            }
        }
        let answer = self.json("check", json!(command), check::SYSTEM_PROMPT.to_owned(), command.clone(), check::schema()).await.map_err(Error::Engine)?;
        let risk = answer["risk"].as_f64().ok_or_else(|| Error::Engine("no risk in the answer".to_owned()))?;
        Ok((risk.clamp(0.0, 1.0), answer["reason"].as_str().unwrap_or_default().to_owned()))
    }

    /// Starts one generated answer to a quick question about the laptop and returns its id: `context` (may be
    /// empty) and what Kith reads of the laptop for it go with the question, and nothing said before does. The
    /// text arrives as `AnswerPiece` signals, then `AnswerDone`, whose error is empty on success.
    async fn ask(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, prompt: String, context: String) -> Result<u32, Error> {
        self.allowed(self.config().features.ask, "ask")?;
        let context = if context.is_empty() { context } else { format!("Context:\n{context}") };
        let seen = self.senses(&prompt).await;
        let context = if seen.is_empty() { context } else if context.is_empty() { seen } else { format!("{context}\n\n{seen}") };
        let verbatim = asks_what_it_can_do(&prompt).then(|| CAN_DO.to_owned());
        Ok(self.answer(emitter, Ask { system: self.ask_system(&prompt), prompt, context, verbatim }))
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

    /// A piece of answer `id`, which starts at byte `offset` of the answer's text.
    #[zbus(signal)]
    async fn answer_piece(emitter: &SignalEmitter<'_>, id: u32, offset: u32, text: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn answer_done(emitter: &SignalEmitter<'_>, id: u32, error: &str) -> zbus::Result<()>;

    /// What Kith is doing, for its face: `idle`, `thinking` or `speaking`.
    #[zbus(property(emits_changed_signal = "false"))]
    fn state(&self) -> &str {
        self.busy.state()
    }

    /// `State` changed.
    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, state: &str) -> zbus::Result<()>;

    /// A new suggestion, `id` in `OpenSuggestions`.
    #[zbus(signal)]
    pub async fn suggestion(emitter: &SignalEmitter<'_>, id: u64, text: &str) -> zbus::Result<()>;

    /// The open suggestions changed: one was shown or answered. Read them again with `OpenSuggestions`.
    #[zbus(signal)]
    async fn suggestions_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_abilities_are_listed_only_for_a_question_about_them() {
        for asked in ["what can you do", "How about web search?", "can you open the browser", "wwhat can you do"] {
            assert!(asks_abilities(asked), "{asked}");
        }
        for asked in ["hi", "thanks", "how much disk is left", "we are connected to the internet?", "what do you mean"] {
            assert!(!asks_abilities(asked), "{asked}");
        }
    }

    #[test]
    fn what_can_you_do_is_answered_by_code() {
        for asked in ["what can you do?", "wwhat can you do", "What are your capabilities", "what do you do"] {
            assert!(asks_what_it_can_do(asked), "{asked}");
        }
        for asked in ["can you open the browser", "how about web search?", "hi", "what do you mean"] {
            assert!(!asks_what_it_can_do(asked), "{asked}");
        }
    }
}
