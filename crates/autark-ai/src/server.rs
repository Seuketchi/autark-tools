//! `org.autark.AI1` on the session bus.

use std::collections::HashSet;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;

use crate::chat::{Chat, Proposal, SuggestionState, TaskState, What};
use crate::config::Ai as Config;
use crate::ollama::Ollama;
use crate::{act, agent, chat, check, events, files, jev, log, memory, route, senses, soul, suggest, tasks, web};

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
    chat: Arc<Mutex<Chat>>,
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
            chat: Arc::new(Mutex::new(Chat::load())),
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

    /// What Kith can see about what `question` is about, as context for the answer (see `senses`): the laptop's own
    /// numbers for a system question, search results for a look-up when search is on, and for anything Kith has no
    /// source for, a line saying so. Empty when the question is about none of these.
    async fn senses(&self, connection: &zbus::Connection, question: &str) -> String {
        let mut seen: Vec<String> = Vec::new();
        for subject in senses::subjects(question) {
            seen.push(match subject {
                senses::Subject::System => senses::system(),
                senses::Subject::Web => self.looked_up(connection, question).await,
                other => senses::unavailable(other).to_owned(),
            });
        }
        seen.join("\n\n")
    }

    /// The short search results for a look-up question, or `None` when search is off or failed. Logged like any call.
    async fn search_found(&self, connection: &zbus::Connection, question: &str) -> Option<String> {
        let config = self.config();
        if !config.web.search || config.local_only {
            return None;
        }
        let start = Instant::now();
        let result = async {
            let key = web::key(connection).await?;
            let query = question.to_owned();
            blocking::unblock(move || web::lookup(&key, &query)).await
        }
        .await;
        log::record("search", "firecrawl", json!(question), &result.clone().map(|text| json!({ "bytes": text.len() })), start.elapsed());
        result.ok().filter(|found| !found.trim().is_empty())
    }

    /// Search results for a look-up question, or why Kith can't search, in the words the model is given.
    async fn looked_up(&self, connection: &zbus::Connection, question: &str) -> String {
        match self.search_found(connection, question).await {
            Some(found) => format!(
                "Web search results for the question. Answer only from these. Say which site you take it from. Copy names and numbers exactly as written, \
                 keeping each label (\"stable\", \"mainline\", \"beta\") with its own number; if they don't answer it, say so.\n{found}"
            ),
            None => senses::unavailable(senses::Subject::Web).to_owned(),
        }
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

    /// `Rank`'s scores from Jev, one noul per item, logged like any call. The query and items leave the laptop,
    /// which is why `[jev] files` and `[jev] settings` are off by default.
    async fn jev_rank(&self, connection: &zbus::Connection, query: &str, items: &[String]) -> Result<Vec<f64>, String> {
        let start = Instant::now();
        let result = async {
            let key = jev::key(connection).await?;
            let state = format!("What the user searched for:\n{query}");
            let questions: serde_json::Map<String, Value> = items
                .iter()
                .enumerate()
                .map(|(n, item)| (n.to_string(), json!({ "type": "noul", "instructions": format!("“{item}” is a likely match for the search.") })))
                .collect();
            let answers = blocking::unblock(move || jev::ask(&key, &state, Value::Object(questions))).await?;
            (0..items.len()).map(|n| jev::noul(&answers, &n.to_string()).map(|score| score.clamp(0.0, 1.0))).collect::<Result<Vec<f64>, String>>()
        }
        .await;
        log::record("rank", "jev", json!({ "query": query, "items": items }), &result.clone().map(|scores| json!({ "scores": scores })), start.elapsed());
        result
    }

    /// `Route` from Jev, logged like any call. `None` when Jev can't route the sentence alone (see `route::jev_parse`).
    /// The sentence leaves the laptop, which is why `[jev] launcher` is off by default.
    async fn jev_route(&self, connection: &zbus::Connection, text: &str, conversation: &str) -> Result<Option<route::Routed>, String> {
        let start = Instant::now();
        let result = async {
            let key = jev::key(connection).await?;
            let (state, questions) = route::jev_questions(text, conversation);
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
    /// `logged` is added to the call log's input. A chat answer grows its entry in the conversation as it's
    /// written, so the Assistant panel can pick it up part way.
    fn answer(&self, emitter: SignalEmitter<'_>, ask: Ask, logged: Value) -> u32 {
        let id = self.next_answer.fetch_add(1, Ordering::Relaxed);
        let emitter = emitter.into_owned();
        let ollama = self.ollama();
        let stopped = self.stopped.clone();
        let busy = self.busy.clone();
        let chat = ask.entry.map(|entry| (self.chat.clone(), entry));
        if let Some((chat, entry)) = &chat {
            let mut chat = lock(chat);
            if let Some(What::Answer { answer, .. }) = chat.get_mut(*entry) {
                *answer = id;
            }
        }
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
            let mut user = if ask.context.is_empty() { ask.prompt.clone() } else { format!("{}\n\nQuestion: {}", ask.context, ask.prompt) };
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
                if let Some((chat, _)) = &chat
                    && let Some(What::Answer { text, .. }) = lock(chat).answer_mut(id)
                {
                    text.push_str(piece);
                }
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
                None => {
                    let mut stopped_early = false;
                    if let Some(web_open) = ask.agent {
                        // One line a step at the top of the answer says what Kith looked at.
                        let (notes, stopped) = agent::gather(
                            &ask.prompt,
                            web_open,
                            agent::seeds(&ask.prompt, web_open),
                            |system, step, schema| {
                                let start = Instant::now();
                                let choice = ollama.json(system, step, schema);
                                log::record("agent", "ollama", json!({ "question": ask.prompt, "step": step }), &choice, start.elapsed());
                                choice
                            },
                            |tool, input| agent::execute(tool, input, web_open),
                            |note| write(&format!("{}\n", agent::shown(note))),
                        );
                        stopped_early = stopped;
                        if !notes.is_empty() {
                            write("\n");
                        }
                        let context = if ask.context.is_empty() { agent::found(&notes) } else { format!("{}\n\n{}", ask.context, agent::found(&notes)) };
                        user = format!("{context}\n\nQuestion: {}", ask.prompt);
                    }
                    if stopped_early { Ok(()) } else { ollama.stream(&ask.system, &user, &mut write) }
                }
            };
            let answer = result.map(|()| json!(text));
            let mut input = json!({ "prompt": ask.prompt, "context": ask.context });
            if let (Some(input), Value::Object(logged)) = (input.as_object_mut(), logged) {
                input.extend(logged);
            }
            log::record(if chat.is_some() { "chat" } else { "ask" }, if ask.verbatim.is_some() { "firecrawl" } else { "ollama" }, input, &answer, start.elapsed());
            announce(if started { busy.change(0, -1) } else { busy.change(-1, 0) });
            let error = answer.err().unwrap_or_default();
            if let Ok(mut stopped) = stopped.lock() {
                stopped.remove(&id);
            }
            if let Some((chat, _)) = &chat {
                let mut chat = lock(chat);
                if let Some(What::Answer { writing, error: why, .. }) = chat.answer_mut(id) {
                    *writing = false;
                    why.clone_from(&error);
                }
                chat.save();
            }
            if let Err(err) = async_io::block_on(Ai::answer_done(&emitter, id, &error)) {
                eprintln!("ai: sending an answer: {err}");
            }
            if chat.is_some()
                && let Err(err) = async_io::block_on(Ai::history_changed(&emitter))
            {
                eprintln!("ai: sending a conversation change: {err}");
            }
        });
        id
    }

    /// Routes `text`, with the conversation so far when there is one.
    async fn routed(&self, emitter: &SignalEmitter<'_>, connection: &zbus::Connection, text: &str, conversation: &str) -> Result<route::Routed, Error> {
        self.allowed(self.config().features.route, "route")?;
        let facts = memory::facts();
        let candidates = route::candidates(text, &facts);
        let config = self.config();
        if config.jev.launcher && !config.local_only && route::jev_can(text, &candidates) {
            self.announce(emitter, self.busy.change(1, 0)).await;
            let routed = self.jev_route(connection, text, conversation).await;
            self.announce(emitter, self.busy.change(-1, 0)).await;
            // Ollama routes what Jev hands over or can't answer, so the launcher never loses its answer.
            match routed {
                Ok(Some(routed)) => return Ok(routed),
                Ok(None) => {}
                Err(err) => eprintln!("ai: {err}; Ollama routes it instead"),
            }
        }
        let user = route::user(text, &candidates, conversation);
        self.announce(emitter, self.busy.change(1, 0)).await;
        let answer = self.json("route", json!(text), route::system_prompt(&facts, text), user, route::schema(&candidates, text)).await;
        self.announce(emitter, self.busy.change(-1, 0)).await;
        answer.and_then(|answer| route::parse(answer, text, &candidates, &facts)).map_err(Error::Engine)
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
    /// as an entry in the conversation, a quiet notification and the `Suggestion` signal.
    pub async fn consider(&self, emitter: &SignalEmitter<'_>, connection: &zbus::Connection, found: Vec<events::Event>) {
        let Some(limits) = self.suggesting() else { return };
        let Some(event) = self.policy().pick(found, suggest::now(), Instant::now(), limits.per_hour) else { return };
        let focus = suggest::focus(connection).await;
        let input = json!({ "kind": event.kind, "facts": event.facts, "offer": event.offer.as_ref().map(|(_, label)| label), "focus": focus });
        let answer = match self.json("suggest", input, suggest::SYSTEM.to_owned(), suggest::user(&event, focus, &memory::facts()), suggest::schema()).await {
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
        let suggestion = What::Suggestion {
            event: event.kind,
            key: event.key(),
            label: event.label.clone(),
            text: text.clone(),
            offer,
            offer_label,
            worth,
            state: SuggestionState::Open,
            result: String::new(),
            notification,
        };
        let entry = self.change(emitter, |chat| chat.push(suggestion)).await;
        if let Err(err) = Ai::suggestion(emitter, entry, &text).await {
            eprintln!("ai: sending a suggestion: {err}");
        }
    }

    /// Changes the conversation, saves it and says so.
    async fn change<T>(&self, emitter: &SignalEmitter<'_>, change: impl FnOnce(&mut Chat) -> T) -> T {
        let result = {
            let mut chat = lock(&self.chat);
            let result = change(&mut chat);
            chat.save();
            result
        };
        if let Err(err) = Ai::history_changed(emitter).await {
            eprintln!("ai: sending a conversation change: {err}");
        }
        result
    }
}

/// What an answer is asked from.
struct Ask {
    system: String,
    prompt: String,
    context: String,
    /// The conversation's entry the answer goes in, for a chat answer.
    entry: Option<u64>,
    /// An answer written already, shown as it is with no model: what a look-up's sources say.
    verbatim: Option<String>,
    /// Gather what the question needs with the tool loop first (see `agent`); whether it may search the web.
    agent: Option<bool>,
}

const ASK_SYSTEM: &str = "You are Kith, the assistant built into the Autark desktop. Answer briefly and plainly. \
                          You can only see the context given with the question and what you remember about the user. \
                          You cannot see the user's calendar, mail, tasks, files, screen or system status, and you cannot look \
                          anything up on the internet, unless it is in the context. When asked for something you can't see or \
                          don't know, say so in one sentence and what you would need. Never invent events, names, numbers, links or news.";

/// An error as the conversation shows it.
fn said(error: &Error) -> String {
    match error {
        Error::Disabled(why) | Error::Engine(why) => why.clone(),
        Error::ZBus(error) => error.to_string(),
    }
}

fn lock(chat: &Mutex<Chat>) -> std::sync::MutexGuard<'_, Chat> {
    chat.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[zbus::interface(name = "org.autark.AI1")]
impl Ai {
    /// Turns a sentence into the actions it asks for, the question it asks and a fact to propose remembering,
    /// as JSON: `{"actions":[{"action":"mute","on":true,"confidence":0.97}],"question":"Why did my build stop?",
    /// "files":["~/autark/log.md"],"remember":"","confidence":0.9,"engine":"ollama"}`.
    async fn route(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, text: String) -> Result<String, Error> {
        let routed = self.routed(&emitter, connection, &text, "").await?;
        serde_json::to_string(&routed).map_err(|err| Error::Engine(err.to_string()))
    }

    /// `Route` as part of the conversation: the message goes into it, routed with what was said before, with
    /// its actions and proposed fact as entries of their own, whose ids come back as `actions_entry` and
    /// `memory_entry` (0 for none). Then `ChatAnswer`, `ChatAct` and `Remember` carry on from here.
    async fn chat(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, text: String) -> Result<String, Error> {
        let conversation = lock(&self.chat).transcript();
        let routed = self.routed(&emitter, connection, &text, &conversation).await;
        let entries = self
            .change(&emitter, |chat| {
                chat.said(&text);
                let routed = match &routed {
                    Ok(routed) => routed,
                    Err(err) => {
                        // Kept, so the conversation shows why there's no answer.
                        chat.push(What::Answer { answer: 0, text: String::new(), files: Vec::new(), writing: false, error: said(err) });
                        return Err(said(err));
                    }
                };
                let actions = routed.actions.iter().map(|chosen| serde_json::to_value(&chosen.action).unwrap_or(Value::Null)).collect::<Vec<_>>();
                let confidence = routed.actions.iter().map(|chosen| chosen.confidence).collect();
                let actions = if actions.is_empty() { 0 } else { chat.push(What::Actions { actions, confidence, results: Vec::new(), skipped: false }) };
                let memory = if routed.remember.is_empty() { 0 } else { chat.push(What::Memory { text: routed.remember.clone(), state: Proposal::Proposed }) };
                let task = match &routed.task {
                    Some(task) => chat.push(What::Task { task: task.clone(), state: TaskState::Proposed, unit: String::new(), next: 0 }),
                    None => 0,
                };
                Ok((actions, memory, task))
            })
            .await;
        let routed = routed?;
        let (actions, memory, task) = entries.map_err(Error::Engine)?;
        let mut json = serde_json::to_value(&routed).map_err(|err| Error::Engine(err.to_string()))?;
        json["actions_entry"] = json!(actions);
        json["memory_entry"] = json!(memory);
        json["task_entry"] = json!(task);
        Ok(json.to_string())
    }

    /// Creates a proposed task, as its card shows it (`task`, JSON, which you may have edited): its folder, its
    /// timer, enabled and started. Returns the timer's unit and its first run, in microseconds since the epoch.
    async fn create_task(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, entry: u64, task: String) -> Result<(String, u64), Error> {
        if !matches!(lock(&self.chat).get_mut(entry), Some(What::Task { state: TaskState::Proposed, .. })) {
            return Err(Error::Engine("that task has been answered already".to_owned()));
        }
        let task: tasks::Task = serde_json::from_str(&task).map_err(|err| Error::Engine(format!("that isn't a task: {err}")))?;
        let task = tasks::check(task).map_err(Error::Engine)?;
        let runner = std::env::current_exe().map_err(|err| Error::Engine(format!("finding autark-ai itself: {err}")))?;
        let created = tasks::create(connection, task.clone(), self.config().tasks, &runner.to_string_lossy()).await.map_err(Error::Engine)?;
        log::record("create_task", "code", json!({ "task": task, "id": created.id }), &Ok(json!({ "unit": created.unit, "next": created.next })), Default::default());
        self.change(&emitter, |chat| {
            if let Some(What::Task { task: kept, state, unit, next }) = chat.get_mut(entry) {
                *kept = task;
                *state = TaskState::Created;
                unit.clone_from(&created.unit);
                *next = created.next;
            }
        })
        .await;
        Ok((created.unit, created.next))
    }

    /// Answers a proposed task with Cancel.
    async fn cancel_task(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, entry: u64) -> Result<(), Error> {
        if !matches!(lock(&self.chat).get_mut(entry), Some(What::Task { state: TaskState::Proposed, .. })) {
            return Err(Error::Engine("that task has been answered already".to_owned()));
        }
        self.change(&emitter, |chat| {
            if let Some(What::Task { state, .. }) = chat.get_mut(entry) {
                *state = TaskState::Cancelled;
            }
        })
        .await;
        Ok(())
    }

    /// `AskFiles` as part of the conversation: answered with what was said before and what Kith remembers, the
    /// answer kept as an entry that grows as it's written. Returns the answer's id, its entry and the files.
    async fn chat_answer(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, question: String, files: Vec<String>) -> Result<(u32, u64, Vec<(String, String)>), Error> {
        self.allowed(self.config().features.ask, "ask")?;
        let (read, outcome) = blocking::unblock(move || files::read(&files)).await;
        let conversation = lock(&self.chat).transcript();
        let entry = self.change(&emitter, |chat| chat.push(What::Answer { answer: 0, text: String::new(), files: outcome.clone(), writing: true, error: String::new() })).await;
        let facts = memory::facts();
        let mut system = format!("{ASK_SYSTEM} You're talking with the user in a running conversation.{}", soul::voice());
        if !facts.is_empty() {
            let facts: Vec<String> = facts.iter().map(|fact| format!("- {fact}")).collect();
            system.push_str(&format!("\nWhat you remember about the user:\n{}", facts.join("\n")));
        }
        let mut context = String::new();
        if !conversation.is_empty() {
            context.push_str(&format!("Conversation so far:\n{conversation}\n\n"));
        }
        context.push_str(&read);
        // A question that is only a look-up shows what its sources say: a small model misread their labels.
        let lookup_only = senses::subjects(&question) == [senses::Subject::Web];
        let found = if lookup_only { self.search_found(connection, &question).await } else { None };
        let verbatim = found.as_deref().and_then(|found| web::quoted(found, 1));
        // A question that needs more than one look gathers its own context with the tool loop, in the answer's thread.
        let agent = (verbatim.is_none() && !lookup_only && agent::wants_agent(&question)).then(|| {
            let config = self.config();
            config.web.search && !config.local_only
        });
        if verbatim.is_none() && agent.is_none() {
            let seen = if lookup_only { senses::unavailable(senses::Subject::Web).to_owned() } else { self.senses(connection, &question).await };
            if !seen.is_empty() {
                context.push_str(&format!("\n{seen}"));
            }
        }
        let logged = json!({ "files": outcome.iter().map(|(path, why)| if why.is_empty() { json!(path) } else { json!({ "path": path, "not_read": why }) }).collect::<Vec<_>>() });
        let id = self.answer(emitter, Ask { system, prompt: question, context: context.trim_end().to_owned(), entry: Some(entry), verbatim, agent }, logged);
        Ok((id, entry, outcome))
    }

    /// Runs the chosen actions (by their place) of a conversation entry, as `Act` does, and keeps how each went.
    async fn chat_act(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, entry: u64, chosen: Vec<u32>) -> Result<Vec<String>, Error> {
        let actions = match lock(&self.chat).get_mut(entry) {
            Some(What::Actions { actions, results, skipped: false, .. }) if results.is_empty() => actions.clone(),
            Some(What::Actions { .. }) => return Err(Error::Engine("these actions have run or been passed by".to_owned())),
            _ => return Err(Error::Engine("there are no such actions".to_owned())),
        };
        let picked: Vec<Value> = chosen.iter().filter_map(|at| actions.get(*at as usize).cloned()).collect();
        let parsed: Vec<route::Action> = serde_json::from_value(Value::Array(picked.clone())).map_err(|err| Error::Engine(format!("not actions from the catalog: {err}")))?;
        let start = Instant::now();
        let ran = act::run(connection, &parsed).await;
        log::record("act", "code", Value::Array(picked), &Ok(json!(ran)), start.elapsed());
        self.change(&emitter, |chat| {
            if let Some(What::Actions { results, .. }) = chat.get_mut(entry) {
                *results = vec![None; actions.len()];
                for (at, outcome) in chosen.iter().zip(&ran) {
                    if let Some(result) = results.get_mut(*at as usize) {
                        *result = Some(outcome.clone());
                    }
                }
            }
        })
        .await;
        Ok(ran)
    }

    /// Answers a proposed fact: saved to memory.toml as `text` (which you may have edited), or not.
    async fn remember(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, entry: u64, text: String, save: bool) -> Result<(), Error> {
        let text = text.trim().to_owned();
        if !matches!(lock(&self.chat).get_mut(entry), Some(What::Memory { state: Proposal::Proposed, .. })) {
            return Err(Error::Engine("that fact has been answered already".to_owned()));
        }
        if save {
            if text.is_empty() {
                return Err(Error::Engine("there's nothing to remember".to_owned()));
            }
            memory::add(&text).map_err(Error::Engine)?;
        }
        self.change(&emitter, |chat| {
            if let Some(What::Memory { text: kept, state }) = chat.get_mut(entry) {
                *state = if save { Proposal::Saved } else { Proposal::NotSaved };
                if save {
                    *kept = text;
                }
            }
        })
        .await;
        Ok(())
    }

    /// Answers a suggestion: `accept` runs its offer through `Act` and returns how it went (empty for done, else
    /// why not); `not_now` passes; `never` also stops this kind and subject being suggested again.
    async fn answer_suggestion(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, entry: u64, answer: String) -> Result<String, Error> {
        let (offer, key, label, notification) = match lock(&self.chat).get(entry) {
            Some(found) => match &found.what {
                What::Suggestion { .. } if suggest::now().saturating_sub(found.time) > suggest::STANDS => return Err(Error::Engine("that suggestion has passed by".to_owned())),
                What::Suggestion { state: SuggestionState::Open, offer, key, label, notification, .. } => (offer.clone(), key.clone(), label.clone(), *notification),
                What::Suggestion { .. } => return Err(Error::Engine("that suggestion has been answered already".to_owned())),
                _ => return Err(Error::Engine("there's no such suggestion".to_owned())),
            },
            None => return Err(Error::Engine("there's no such suggestion".to_owned())),
        };
        let (state, result) = match answer.as_str() {
            "accept" => {
                let offer = offer.ok_or_else(|| Error::Engine("that suggestion offers nothing".to_owned()))?;
                let action: route::Action = serde_json::from_value(offer.clone()).map_err(|err| Error::Engine(format!("not an action: {err}")))?;
                let start = Instant::now();
                let ran = act::run(connection, &[action]).await;
                log::record("act", "code", json!([offer]), &Ok(json!(ran)), start.elapsed());
                (SuggestionState::Accepted, ran.into_iter().next().unwrap_or_default())
            }
            "not_now" => (SuggestionState::NotNow, String::new()),
            "never" => {
                self.policy().never(&key, &label);
                (SuggestionState::Never, String::new())
            }
            _ => return Err(Error::Engine(format!("“{answer}” isn't an answer: accept, not_now or never"))),
        };
        self.change(&emitter, |chat| {
            if let Some(What::Suggestion { state: kept, result: why, .. }) = chat.get_mut(entry) {
                *kept = state;
                why.clone_from(&result);
            }
        })
        .await;
        if notification != 0 {
            suggest::close(connection, notification).await;
        }
        Ok(result)
    }

    /// The suggestions still open and standing, newest last, as a JSON array of conversation entries.
    async fn open_suggestions(&self) -> Result<String, Error> {
        let now = suggest::now();
        let mut chat = lock(&self.chat);
        let open: Vec<&chat::Entry> =
            chat.entries().iter().filter(|entry| matches!(entry.what, What::Suggestion { state: SuggestionState::Open, .. }) && now.saturating_sub(entry.time) <= suggest::STANDS).collect();
        serde_json::to_string(&open).map_err(|err| Error::Engine(err.to_string()))
    }

    /// What you said never to suggest again: each one's key and how to name it.
    async fn never_suggested(&self) -> Vec<(String, String)> {
        self.policy().nevers().iter().map(|never| (never.key.clone(), never.label.clone())).collect()
    }

    /// Lets something you said never to suggest be suggested again.
    async fn suggest_again(&self, key: String) {
        self.policy().again(&key);
    }

    /// The conversation, as a JSON array of entries.
    async fn history(&self) -> Result<String, Error> {
        serde_json::to_string(lock(&self.chat).entries()).map_err(|err| Error::Engine(err.to_string()))
    }

    /// Empties the conversation.
    async fn clear_history(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.change(&emitter, Chat::clear).await;
    }

    /// How relevant each item is to the query, 0 to 1, in the items' order.
    async fn rank(&self, #[zbus(connection)] connection: &zbus::Connection, query: String, items: Vec<String>) -> Result<Vec<f64>, Error> {
        self.rank_for(connection, String::new(), query, items).await
    }

    /// `Rank` for a caller that has its own Jev switch: `source` is "files" or "settings".
    async fn rank_for(&self, #[zbus(connection)] connection: &zbus::Connection, source: String, query: String, items: Vec<String>) -> Result<Vec<f64>, Error> {
        self.allowed(self.config().features.rank, "rank")?;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let config = self.config();
        let jev = match source.as_str() {
            "files" => config.jev.files,
            "settings" => config.jev.settings,
            _ => false,
        };
        if jev && !config.local_only {
            // If Jev can't answer, Ollama does, so search never loses its ranking.
            match self.jev_rank(connection, &query, &items).await {
                Ok(scores) => return Ok(scores),
                Err(err) => eprintln!("ai: {err}; Ollama ranks it instead"),
            }
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

    /// A web search through Firecrawl: titles, addresses and snippets of the best results, as text. The query leaves
    /// the laptop, which is why `[web] search` is off by default; it is an error unless that is on and `local_only` is not.
    async fn search(&self, #[zbus(connection)] connection: &zbus::Connection, query: String) -> Result<String, Error> {
        let config = self.config();
        if !config.enabled {
            return Err(Error::Disabled("AI is off: set `enabled = true` in ai.toml".to_owned()));
        }
        if !config.web.search || config.local_only {
            return Err(Error::Disabled("search is off in ai.toml".to_owned()));
        }
        let start = Instant::now();
        let result = async {
            let key = web::key(connection).await?;
            let query = query.clone();
            blocking::unblock(move || web::search(&key, &query)).await
        }
        .await;
        log::record("search", "firecrawl", json!(query), &result.clone().map(|text| json!({ "bytes": text.len() })), start.elapsed());
        result.map_err(Error::Engine)
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

    /// Starts a generated answer and returns its id. The text arrives as `AnswerPiece` signals,
    /// then `AnswerDone`, whose error is empty on success.
    async fn ask(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, prompt: String, context: String) -> Result<u32, Error> {
        self.allowed(self.config().features.ask, "ask")?;
        let context = if context.is_empty() { context } else { format!("Context:\n{context}") };
        let seen = self.senses(connection, &prompt).await;
        let context = if seen.is_empty() { context } else if context.is_empty() { seen } else { format!("{context}\n\n{seen}") };
        Ok(self.answer(emitter, Ask { system: format!("{ASK_SYSTEM}{}", soul::voice()), prompt, context, entry: None, verbatim: None, agent: None }, json!({})))
    }

    /// `Ask` with files for context, as `Route` picked them: reads the ones Kith may read (see `files`) and
    /// says how each went, an empty reason for read, before the answer streams.
    async fn ask_files(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, prompt: String, files: Vec<String>) -> Result<(u32, Vec<(String, String)>), Error> {
        self.allowed(self.config().features.ask, "ask")?;
        let (context, outcome) = blocking::unblock(move || files::read(&files)).await;
        let read = json!(outcome.iter().map(|(path, why)| if why.is_empty() { json!(path) } else { json!({ "path": path, "not_read": why }) }).collect::<Vec<_>>());
        let context = if context.is_empty() { context } else { format!("Context:\n{}", context.trim_end()) };
        let seen = self.senses(connection, &prompt).await;
        let context = if seen.is_empty() { context } else if context.is_empty() { seen } else { format!("{context}\n\n{seen}") };
        Ok((self.answer(emitter, Ask { system: format!("{ASK_SYSTEM}{}", soul::voice()), prompt, context, entry: None, verbatim: None, agent: None }, json!({ "files": read })), outcome))
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

    /// A new suggestion, as conversation entry `entry`.
    #[zbus(signal)]
    pub async fn suggestion(emitter: &SignalEmitter<'_>, entry: u64, text: &str) -> zbus::Result<()>;

    /// The conversation changed: read it again with `History`.
    #[zbus(signal)]
    async fn history_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
