//! `org.autark.AI1` on the session bus.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;

use crate::chat::{Chat, Proposal, SuggestionState, TaskState, What};
use crate::config::Ai as Config;
use crate::ollama::Ollama;
use crate::{act, agent, agents, board, calendar, chat, check, events, files, jev, log, mail, memory, recall, route, senses, skills, soul, suggest, tasks, web};

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
    /// The other agents' conversations, read when first needed.
    chats: Mutex<HashMap<String, Arc<Mutex<Chat>>>>,
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
            chats: Mutex::default(),
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

    /// The instructions for a generated answer: the rules, what Kith can do when `question` asks, how it sounds, then the soul.
    fn ask_system(&self, question: &str, conversation: bool) -> String {
        let talking = if conversation { " You're talking with the user in a running conversation." } else { "" };
        let can = if asks_abilities(question) { abilities(&self.config()) } else { String::new() };
        format!("{ASK_SYSTEM}{talking}{can}{ASK_VOICE}{}", soul::voice())
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
        self.senses_for(connection, question, None).await
    }

    /// `senses`, for an agent that may look at only what its job needs: a subject it may not look at is
    /// answered with a line saying so, so it doesn't guess.
    async fn senses_for(&self, connection: &zbus::Connection, question: &str, agent: Option<&agents::Agent>) -> String {
        let mut seen: Vec<String> = Vec::new();
        for subject in senses::subjects(question) {
            if let Some(agent) = agent
                && !agent.allows(subject.ability())
            {
                seen.push(format!("You are not set up to look at the user's {}. Say so in one sentence, and that Kith itself can; do not guess.", subject.ability()));
                continue;
            }
            seen.push(match subject {
                senses::Subject::System => senses::system(),
                senses::Subject::Web => self.looked_up(connection, question).await,
                senses::Subject::Calendar => match self.calendar_found(connection, question).await {
                    Some(Ok(agenda)) => format!("The user's calendar, read just now (answer only from this):\n{agenda}"),
                    Some(Err(why)) => format!("Kith could not read the user's calendar just now: {why}. Tell the user that in one sentence. Do not make up events."),
                    None => senses::unavailable(senses::Subject::Calendar).to_owned(),
                },
                senses::Subject::Tasks => match self.board_found().await {
                    Ok(board) => format!("The user's GitHub project tasks, read just now (answer only from this):\n{board}"),
                    Err(why) => format!("Kith could not read the user's tasks just now: {why}. Tell the user that in one sentence. Do not make up tasks."),
                },
                senses::Subject::Mail => match self.mail_found(connection, question).await {
                    Ok(inbox) if mail::wants_body(question) => format!("From the user's inbox, read just now and left unread (answer only from this):\n{inbox}"),
                    Ok(inbox) => format!("The user's inbox, read just now (answer only from this; only who each message is from and its subject are known):\n{inbox}"),
                    Err(why) => format!("Kith could not read the user's email just now: {why}. Tell the user that in one sentence. Do not make up messages."),
                },
                senses::Subject::Recall => {
                    let question = question.to_owned();
                    format!("What Kith found in the user's notes and earlier conversations (answer only from this):\n{}", blocking::unblock(move || recall::search(&question)).await)
                }
            });
        }
        seen.join("\n\n")
    }

    /// The calendar for a question about it, read from the feed you gave Kith: `None` when no address is set,
    /// else the agenda or why it couldn't be read. Logged like any call, without the address, which is the key.
    async fn calendar_found(&self, connection: &zbus::Connection, question: &str) -> Option<Result<String, String>> {
        let address = calendar::url(connection).await.ok()?;
        let start = Instant::now();
        let asked = question.to_owned();
        let result = blocking::unblock(move || calendar::answer(&address, &asked)).await;
        log::record("calendar", "ical", json!(question), &result.clone().map(|text| json!({ "bytes": text.len() })), start.elapsed());
        Some(result)
    }

    /// The user's GitHub project tasks, read with `gh`. Logged like any call, with how much came back and nothing of it.
    async fn board_found(&self) -> Result<String, String> {
        let start = Instant::now();
        let result = blocking::unblock(board::answer).await;
        log::record("tasks", "gh", json!(null), &result.clone().map(|text| json!({ "bytes": text.len() })), start.elapsed());
        result
    }

    /// The inbox for a question about mail, read through the Mail service. Logged like any call, with how much came
    /// back and nothing of the messages.
    async fn mail_found(&self, connection: &zbus::Connection, question: &str) -> Result<String, String> {
        let start = Instant::now();
        let result = mail::answer(connection, question).await;
        log::record("mail", "imap", json!(question), &result.clone().map(|text| json!({ "bytes": text.len() })), start.elapsed());
        result
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
        let chat = ask.entry.map(|entry| (ask.chat.clone().unwrap_or_else(|| self.chat.clone()), entry));
        let owner = ask.owner.clone();
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
            let mut proposal: Option<skills::Skill> = None;
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
                            ask.skill.as_ref().map_or_else(|| agent::seeds(&ask.prompt, web_open), |skill| agent::skill_seeds(skill, web_open)),
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
                        // Two steps that worked, for a question no skill covers yet, are offered as one.
                        if ask.skill.is_none() && !stopped {
                            proposal = skills::propose(&ask.prompt, &notes, &skills::all());
                        }
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
                if let Some(skill) = proposal.take()
                    && error.is_empty()
                {
                    chat.push(What::Skill { skill, state: Proposal::Proposed });
                }
                chat.save();
            }
            if let Err(err) = async_io::block_on(Ai::answer_done(&emitter, id, &error)) {
                eprintln!("ai: sending an answer: {err}");
            }
            if chat.is_some() {
                if !owner.is_empty()
                    && let Err(err) = async_io::block_on(Ai::agent_changed(&emitter, &owner))
                {
                    eprintln!("ai: sending an agent's conversation change: {err}");
                }
                if (owner.is_empty() || owner == agents::KITH)
                    && let Err(err) = async_io::block_on(Ai::history_changed(&emitter))
                {
                    eprintln!("ai: sending a conversation change: {err}");
                }
            }
        });
        id
    }

    /// Routes `text`, with the conversation so far when there is one.
    async fn routed(&self, emitter: &SignalEmitter<'_>, connection: &zbus::Connection, text: &str, conversation: &str) -> Result<route::Routed, Error> {
        self.allowed(self.config().features.route, "route")?;
        // A clear one-off reminder is read by code: instant, and the time is never a model's guess.
        if let Some(routed) = route::reminder(text, chrono::Local::now().naive_local()) {
            return Ok(routed);
        }
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
        answer
            .and_then(|answer| route::parse(answer, text, &candidates, &facts))
            .map(|mut routed| {
                // A task that names a saved skill runs that skill's steps each time.
                if let Some(task) = &mut routed.task
                    && let Some(skill) = skills::matching(text, &skills::all())
                {
                    task.skill = skill.name;
                }
                routed
            })
            .map_err(Error::Engine)
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

    /// An agent's conversation: Kith's own, or the agent's, read when first needed.
    fn chat_of(&self, agent: &agents::Agent) -> Option<Arc<Mutex<Chat>>> {
        if agent.id == agents::KITH {
            return Some(self.chat.clone());
        }
        let mut chats = self.chats.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(chat) = chats.get(&agent.id) {
            return Some(chat.clone());
        }
        let chat = Arc::new(Mutex::new(Chat::of(agents::chat_path(&agent.id)?, &agent.name)));
        chats.insert(agent.id.clone(), chat.clone());
        Some(chat)
    }

    /// The instructions for an agent's answer: the rules, its job, how it sounds, then its soul. Kith's own are
    /// the launcher's, with what it remembers.
    fn agent_system(&self, agent: &agents::Agent, question: &str) -> String {
        if agent.id == agents::KITH {
            let mut system = self.ask_system(question, true);
            let facts = memory::facts();
            if !facts.is_empty() {
                let facts: Vec<String> = facts.iter().map(|fact| format!("- {fact}")).collect();
                system.push_str(&format!("\nWhat you remember about the user:\n{}", facts.join("\n")));
            }
            return system;
        }
        let job = if agent.job.is_empty() { String::new() } else { format!(" Your job: {}", agent.job) };
        // A skill is how the agent works, so it replaces the rule to answer in a sentence or two.
        let skills: Vec<String> = agent.skills.iter().filter_map(|name| agents::skill_text(name).map(|text| format!("\nYour skill “{name}”, which is how you work the user's questions; it never changes the limits below:\n{text}"))).collect();
        let rules = if skills.is_empty() { AGENT_BRIEF } else { AGENT_CHAT };
        format!("You are {}, one of the agents on the user's Autark desktop, in a running conversation with the user.{job}{}{rules}{AGENT_LIMITS}{ASK_VOICE}{}", agent.name, skills.join(""), soul::with(&agent.soul))
    }

    /// Changes an agent's conversation, saves it and says so.
    async fn change_agent<T>(&self, emitter: &SignalEmitter<'_>, agent: &agents::Agent, chat: &Mutex<Chat>, change: impl FnOnce(&mut Chat) -> T) -> T {
        let result = {
            let mut chat = lock(chat);
            let result = change(&mut chat);
            chat.save();
            result
        };
        if let Err(err) = Ai::agent_changed(emitter, &agent.id).await {
            eprintln!("ai: sending an agent's conversation change: {err}");
        }
        if agent.id == agents::KITH
            && let Err(err) = Ai::history_changed(emitter).await
        {
            eprintln!("ai: sending a conversation change: {err}");
        }
        result
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
    /// A saved skill the question calls for, whose steps are the loop's first looks.
    skill: Option<skills::Skill>,
    /// The conversation the entry is in, when it isn't Kith's own, and whose it is: an agent's id, or empty.
    chat: Option<Arc<Mutex<Chat>>>,
    owner: String,
}

const ASK_SYSTEM: &str = "You are Kith, the assistant built into the Autark desktop. Answer the question that was asked, in one or two plain sentences, unless the user asks for a list or more detail: then give it, \
                          one item to a line, each starting with \"- \". \
                          You know only what is given with the question and what you remember about the user. When something asked for isn't \
                          given, say so in one sentence and what you would need. Never invent events, names, numbers, links or news, and never \
                          claim an ability you haven't been told you have.";

/// How an agent without a skill answers.
const AGENT_BRIEF: &str = " Answer what is asked in one or two plain sentences, unless the user asks for a list or more detail: then give it, \
                            one item to a line, each starting with \"- \".";
/// How an agent with a skill answers: as its skill says, in the size of a chat message.
const AGENT_CHAT: &str = " Follow your skill. Keep each reply short enough to read in a chat: a few sentences, or a short list.";
/// What every agent but Kith is told it can't do, so a small model doesn't claim a power it hasn't got.
const AGENT_LIMITS: &str = " You know only what the user and the conversation so far have told you. When something asked for isn't \
                            given, say so in one sentence and what you would need. Never invent events, names, numbers, links or news. \
                            You can't send, change or run anything. You see only what you are given with the question: if asked about something you were not given, say so and say what you can do instead.";

/// How Kith sounds, kept apart from the soul so a small model gets the same few rules whatever the soul says. It
/// never names "Done.": the panel marks an action done itself, and a small model repeats whatever it is told not to say.
const ASK_VOICE: &str = "\nDon't say \"sir\" or \"ma'am\" unless your soul below asks for it, and never end with a question or an offer: reply to a greeting with a greeting. \
                         If the user asks what you meant, explain your last reply in other words and give the reason; don't repeat it or change the subject.";

/// Whether a question is about what Kith can do. The list of abilities goes in only then: a small model that has it
/// in front of it offers those things, or reports a battery level, to a plain "hi".
fn asks_abilities(question: &str) -> bool {
    let question = question.to_lowercase();
    ["can you", "could you", "able to", "what do you do", "what can", "capab", "abilit", "feature", "how about", "do you have", "help me"].iter().any(|ask| question.contains(ask))
}

/// Whether a question is plainly "what can you do". That has one answer, so code gives it (`can_do`) instead of a model
/// that took 30 s to describe it wrongly.
fn asks_what_it_can_do(question: &str) -> bool {
    let question = question.to_lowercase();
    ["what can you do", "what do you do", "what are you able", "what are you capable", "your capabilities", "what can kith do"].iter().any(|ask| question.contains(ask))
}

/// What Kith says it can do, as far as the settings allow.
fn can_do(config: &Config) -> String {
    let web = if config.web.search && !config.local_only { "I can also search the web." } else { "Web search is switched off." };
    format!(
        "I can propose actions for you to tick: volume, brightness, mute, Wi-Fi, Focus, night light, lock, sleep, and opening an app or a link. \
         I can look at this laptop's disk, battery, memory and apps, and read your calendar, mail, tasks, notes and files in your home folder \
         (calendar and mail once their accounts are connected). I remember a fact once you approve it, and run tasks on a schedule. {web}"
    )
}

/// What Kith can do, as far as the settings allow, so the model neither denies an ability it has nor makes one up.
fn abilities(config: &Config) -> String {
    let web = if config.web.search && !config.local_only { "Search the web." } else { "Cannot search the web: it is switched off." };
    format!(
        "\nWhat Kith can do:\n\
         - Propose actions for the user to tick: volume, brightness, mute, Wi-Fi, Focus, night light, lock, sleep, open an app or a link.\n\
         - Look at this laptop: disk, battery, memory and what apps are using.\n\
         - Read the user's calendar, mail, tasks and notes, and files in their home folder.\n\
         - Remember a fact once the user approves it, and run tasks on a schedule.\n\
         - {web}"
    )
}

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
        let mut system = self.ask_system(&question, true);
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
        // A question that is only about the calendar is answered from the events themselves: no model to invent one.
        let calendar_only = senses::subjects(&question) == [senses::Subject::Calendar];
        let agenda = if calendar_only { self.calendar_found(connection, &question).await.and_then(Result::ok) } else { None };
        // The same for mail: the list of who wrote and what about, from the inbox itself.
        let mail_only = senses::subjects(&question) == [senses::Subject::Mail] && !mail::wants_body(&question);
        let inbox = if mail_only { self.mail_found(connection, &question).await.ok() } else { None };
        // And for the task board: the project items themselves.
        let board_only = senses::subjects(&question) == [senses::Subject::Tasks];
        let items = if board_only { self.board_found().await.ok() } else { None };
        let verbatim = found.as_deref().and_then(|found| web::quoted(found, 1)).or(agenda).or(inbox).or(items).or_else(|| asks_what_it_can_do(&question).then(|| can_do(&self.config())));
        // A question that needs more than one look gathers its own context with the tool loop, in the answer's thread.
        let skill = if verbatim.is_none() { skills::matching(&question, &skills::all()) } else { None };
        let agent = (verbatim.is_none() && !lookup_only && !calendar_only && !mail_only && !board_only && (skill.is_some() || agent::wants_agent(&question))).then(|| {
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
        let id = self.answer(emitter, Ask { system, prompt: question, context: context.trim_end().to_owned(), entry: Some(entry), verbatim, agent, skill, chat: None, owner: String::new() }, logged);
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

    /// Answers a proposed skill: kept as a file in the skills folder under `name` (which you may have changed), or not.
    async fn save_skill(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, entry: u64, name: String, save: bool) -> Result<(), Error> {
        let mut skill = match lock(&self.chat).get_mut(entry) {
            Some(What::Skill { skill, state: Proposal::Proposed }) => skill.clone(),
            Some(What::Skill { .. }) => return Err(Error::Engine("that skill has been answered already".to_owned())),
            _ => return Err(Error::Engine("there's no such skill".to_owned())),
        };
        if save {
            skill.name = name.trim().to_owned();
            blocking::unblock({
                let skill = skill.clone();
                move || skills::save(&skill)
            })
            .await
            .map_err(Error::Engine)?;
        }
        self.change(&emitter, |chat| {
            if let Some(What::Skill { skill: kept, state }) = chat.get_mut(entry) {
                *state = if save { Proposal::Saved } else { Proposal::NotSaved };
                if save {
                    *kept = skill;
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

    /// The team, as JSON, Kith first: `[{"id","name","colour","job","last","time","writing"}]`. `last` is what was
    /// said last in its conversation and `time` when, in seconds since the epoch (0 for nothing yet).
    async fn agents(&self) -> Result<String, Error> {
        let rows: Vec<Value> = agents::all()
            .into_iter()
            .map(|agent| {
                let (last, time, writing) = match self.chat_of(&agent) {
                    Some(chat) => {
                        let mut chat = lock(&chat);
                        let (last, time) = chat.last_said();
                        (last, time, chat.writing())
                    }
                    None => (String::new(), 0, false),
                };
                json!({ "id": agent.id, "name": agent.name, "colour": agent.colour, "job": agent.job, "last": last, "time": time, "writing": writing })
            })
            .collect();
        serde_json::to_string(&rows).map_err(|err| Error::Engine(err.to_string()))
    }

    /// Adds an agent to the team, and returns its id. `can` is what it may look at: any of mail, calendar, tasks,
    /// notes, web and laptop. Everything it looks at is read-only. `skills` are skill names, as `SKILL.md` folders in
    /// `~/.config/autark-ai/skills-md/` or `~/.claude/skills/`: their text is part of the agent's instructions.
    async fn create_agent(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, name: String, job: String, soul: String, can: Vec<String>, skills: Vec<String>) -> Result<String, Error> {
        let agent = agents::create(&name, &job, &soul, &can, &skills).map_err(Error::Engine)?;
        if let Err(err) = Ai::agents_changed(&emitter).await {
            eprintln!("ai: sending a team change: {err}");
        }
        Ok(agent.id)
    }

    /// Removes an agent and its conversation.
    async fn delete_agent(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String) -> Result<(), Error> {
        agents::delete(&id).map_err(Error::Engine)?;
        self.chats.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&id);
        if let Err(err) = Ai::agents_changed(&emitter).await {
            eprintln!("ai: sending a team change: {err}");
        }
        Ok(())
    }

    /// An agent's conversation, as `History` gives Kith's.
    async fn agent_history(&self, id: String) -> Result<String, Error> {
        let agent = agents::get(&id).ok_or_else(|| Error::Engine(format!("there is no agent {id}")))?;
        let chat = self.chat_of(&agent).ok_or_else(|| Error::Engine(format!("there is no agent {id}")))?;
        serde_json::to_string(lock(&chat).entries()).map_err(|err| Error::Engine(err.to_string()))
    }

    /// Says something to an agent. The answer is kept in its conversation as an entry that grows as it's written,
    /// and comes as `AnswerPiece` signals too. Returns the answer's id and its entry.
    async fn agent_say(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String, text: String) -> Result<(u32, u64), Error> {
        self.allowed(self.config().features.ask, "ask")?;
        let agent = agents::get(&id).ok_or_else(|| Error::Engine(format!("there is no agent {id}")))?;
        let chat = self.chat_of(&agent).ok_or_else(|| Error::Engine(format!("there is no agent {id}")))?;
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err(Error::Engine("there is nothing to say".to_owned()));
        }
        let conversation = lock(&chat).transcript();
        let entry = self
            .change_agent(&emitter, &agent, &chat, |chat| {
                chat.said(&text);
                chat.push(What::Answer { answer: 0, text: String::new(), files: Vec::new(), writing: true, error: String::new() })
            })
            .await;
        let system = self.agent_system(&agent, &text);
        let mut context = if conversation.is_empty() { String::new() } else { format!("Conversation so far:\n{conversation}") };
        // What the agent may look at that the question is about, read now.
        if agent.id != agents::KITH {
            let seen = self.senses_for(connection, &text, Some(&agent)).await;
            if !seen.is_empty() {
                context.push_str(&format!("\n\n{seen}"));
            }
        }
        let answer =
            self.answer(emitter, Ask { system, prompt: text, context, entry: Some(entry), verbatim: None, agent: None, skill: None, chat: Some(chat), owner: agent.id }, json!({ "agent": id }));
        Ok((answer, entry))
    }

    /// Clears an agent's conversation.
    async fn clear_agent(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String) -> Result<(), Error> {
        let agent = agents::get(&id).ok_or_else(|| Error::Engine(format!("there is no agent {id}")))?;
        let chat = self.chat_of(&agent).ok_or_else(|| Error::Engine(format!("there is no agent {id}")))?;
        self.change_agent(&emitter, &agent, &chat, Chat::clear).await;
        Ok(())
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
        Ok(self.answer(
            emitter,
            Ask {
                system: self.ask_system(&prompt, false),
                verbatim: asks_what_it_can_do(&prompt).then(|| can_do(&self.config())),
                prompt,
                context,
                entry: None,
                agent: None,
                skill: None,
                chat: None,
                owner: String::new(),
            },
            json!({}),
        ))
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
        Ok((
            self.answer(
                emitter,
                Ask {
                    system: self.ask_system(&prompt, false),
                    verbatim: asks_what_it_can_do(&prompt).then(|| can_do(&self.config())),
                    prompt,
                    context,
                    entry: None,
                    agent: None,
                    skill: None,
                    chat: None,
                    owner: String::new(),
                },
                json!({ "files": read }),
            ),
            outcome,
        ))
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

    /// An agent's conversation changed: an entry came, or an answer finished.
    #[zbus(signal)]
    async fn agent_changed(emitter: &SignalEmitter<'_>, id: &str) -> zbus::Result<()>;

    /// An agent was added or removed.
    #[zbus(signal)]
    async fn agents_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// The conversation changed: read it again with `History`.
    #[zbus(signal)]
    async fn history_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
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
        let mut config = Config::default();
        assert!(can_do(&config).ends_with("Web search is switched off."));
        config.web.search = true;
        assert!(can_do(&config).ends_with("I can also search the web."));
    }

    #[test]
    fn the_abilities_follow_the_web_switch() {
        let mut config = Config::default();
        assert!(abilities(&config).contains("Cannot search the web"));
        config.web.search = true;
        assert!(abilities(&config).ends_with("- Search the web."));
        config.local_only = true;
        assert!(abilities(&config).contains("Cannot search the web"));
    }
}
