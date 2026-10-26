//! `org.autark.Agents1`: the team's API. It decides which agent answers, what the agent may look at and what it is told;
//! `autark-ai` reads what it is allowed to read and runs the model. A client (the roster, later the launcher or the bar)
//! talks to this and nothing else.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;

use crate::agents::{self, Agent, KITH};
use crate::ai::{self, AiProxy};
use crate::chat::{self, Chat, FactState, What};
use crate::events::Events;
use crate::memory;
use crate::prompt;
use crate::remember;

pub const PATH: &str = "/org/autark/Agents1";

/// How long an answer may go without a word before it is given up on: a model on a busy laptop can take a while to
/// start, but not this long.
const IDLE: Duration = Duration::from_secs(300);
/// How many words that arrive before the call that started them has returned are kept to be matched.
const EARLY: usize = 256;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.autark.Agents1.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
}

fn failed<T>(why: impl Into<String>) -> Result<T, Error> {
    Err(Error::Failed(why.into()))
}

/// An answer being written by an agent here.
struct Pending {
    agent: String,
    heard: Instant,
}

enum Heard {
    Piece(String),
    Done(String),
}

/// A signal from `autark-ai`, as the listener gets it.
enum Told {
    Piece(u32, u32, String),
    Done(u32, String),
    /// Kith's own conversation changed.
    Kith,
}

#[derive(Default)]
struct Core {
    chats: HashMap<String, Chat>,
    pending: HashMap<u32, Pending>,
    /// What `autark-ai` said of an answer before its id reached `say`.
    early: VecDeque<(u32, Heard)>,
}

pub struct Shared {
    core: Mutex<Core>,
    events: Events,
}

impl Shared {
    pub fn new(events: Events) -> Self {
        Self { core: Mutex::new(Core::default()), events }
    }

    fn core(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn prune(&self) {
        self.events.prune(chat::now());
    }
}

impl Core {
    fn chat(&mut self, agent: &Agent) -> &mut Chat {
        self.chats.entry(agent.id.clone()).or_insert_with(|| Chat::load(agents::chat_path(&agent.id).unwrap_or_default(), &agent.name))
    }

    /// Gives an answer its id, and applies what was heard of it already. Returns how it ended, if that was heard.
    fn register(&mut self, answer: u32, agent: &Agent, entry: u64) -> Option<(String, &'static str, String)> {
        if let Some(What::Answer { answer: id, .. }) = self.chat(agent).get_mut(entry) {
            *id = answer;
        }
        self.pending.insert(answer, Pending { agent: agent.id.clone(), heard: Instant::now() });
        let mut finished = None;
        let early = std::mem::take(&mut self.early);
        for (id, heard) in early {
            if id != answer {
                self.early.push_back((id, heard));
                continue;
            }
            match heard {
                Heard::Piece(text) => self.append(answer, &text),
                Heard::Done(error) => finished = self.finish(answer, &error),
            }
        }
        finished
    }

    fn append(&mut self, answer: u32, piece: &str) {
        let Some(pending) = self.pending.get_mut(&answer) else { return };
        pending.heard = Instant::now();
        let agent = pending.agent.clone();
        if let Some(chat) = self.chats.get_mut(&agent)
            && let Some(What::Answer { text, .. }) = chat.answer_mut(answer)
        {
            text.push_str(piece);
        }
    }

    /// Ends an answer, with `error` empty when it went well, and returns the agent it was for and what to log.
    fn finish(&mut self, answer: u32, error: &str) -> Option<(String, &'static str, String)> {
        let pending = self.pending.remove(&answer)?;
        let chat = self.chats.get_mut(&pending.agent)?;
        let mut said = String::new();
        let mut well = false;
        if let Some(What::Answer { text, writing, error: why, .. }) = chat.answer_mut(answer) {
            *writing = false;
            if !error.is_empty() {
                *why = error.to_owned();
            } else if text.is_empty() {
                *why = "No answer came back.".to_owned();
            }
            well = why.is_empty();
            said = if well { text.clone() } else { why.clone() };
        }
        chat.save();
        Some((pending.agent, if well { "answered" } else { "failed" }, said))
    }

    fn hear(&mut self, answer: u32, heard: Heard) -> Option<(String, &'static str, String)> {
        if !self.pending.contains_key(&answer) {
            self.early.push_back((answer, heard));
            while self.early.len() > EARLY {
                self.early.pop_front();
            }
            return None;
        }
        match heard {
            Heard::Piece(text) => {
                self.append(answer, &text);
                None
            }
            Heard::Done(error) => self.finish(answer, &error),
        }
    }
}

pub struct Agents {
    pub shared: Arc<Shared>,
}

fn find(id: &str) -> Result<Agent, Error> {
    agents::get(id).ok_or_else(|| Error::Failed(format!("there is no agent {id}")))
}

/// The question to answer in Kith's own conversation, and its files, from how `Chat` routed what was said. Actions
/// are run from the launcher's panel, where Kith shows what it will do and asks first.
fn kith_question(routed: &str, said: &str) -> Result<(String, Vec<String>), String> {
    let routed: Value = serde_json::from_str(routed).map_err(|err| format!("Kith's routing wasn't understood: {err}"))?;
    let question = routed["question"].as_str().unwrap_or_default().trim();
    let files: Vec<String> = routed["files"].as_array().into_iter().flatten().filter_map(|file| file.as_str().map(str::to_owned)).collect();
    if question.is_empty() {
        if routed["actions"].as_array().is_some_and(|actions| !actions.is_empty()) {
            return Err("That is something for Kith to do, and it asks first in the launcher's panel. Ask it there.".to_owned());
        }
        return Ok((said.to_owned(), files));
    }
    Ok((question.to_owned(), files))
}

/// What was said last, when, and whether an answer is being written, from Kith's conversation.
fn kith_last(history: &str) -> (String, u64, bool) {
    let entries: Vec<Value> = serde_json::from_str(history).unwrap_or_default();
    let writing = entries.iter().any(|entry| entry["kind"] == "answer" && entry["writing"] == true);
    let (last, time) = entries
        .iter()
        .rev()
        .find_map(|entry| match entry["kind"].as_str()? {
            "you" => Some((entry["text"].as_str()?.to_owned(), entry["time"].as_u64().unwrap_or(0))),
            "answer" => Some((entry["text"].as_str().filter(|text| !text.is_empty())?.to_owned(), entry["time"].as_u64().unwrap_or(0))),
            _ => None,
        })
        .unwrap_or_default();
    (last, time, writing)
}

async fn ai(connection: &zbus::Connection) -> Result<AiProxy<'static>, Error> {
    Ok(AiProxy::new(connection).await?)
}

fn engine(error: zbus::Error) -> Error {
    Error::Failed(ai::said(&error))
}

#[zbus::interface(name = "org.autark.Agents1")]
impl Agents {
    /// The team as JSON, Kith first: `[{"id","name","colour","job","last","time","writing"}]`. `last` is what was said
    /// last in its conversation and `time` when, in seconds since the epoch (0 for nothing yet).
    async fn agents(&self, #[zbus(connection)] connection: &zbus::Connection) -> Result<String, Error> {
        let team = agents::all();
        let kith = match team.iter().any(|agent| agent.id == KITH) {
            true => match ai(connection).await {
                Ok(proxy) => proxy.history().await.map(|history| kith_last(&history)).unwrap_or_default(),
                Err(_) => Default::default(),
            },
            false => Default::default(),
        };
        let mut core = self.shared.core();
        let rows: Vec<Value> = team
            .iter()
            .map(|agent| {
                let (last, time, writing) = if agent.id == KITH {
                    kith.clone()
                } else {
                    let chat = core.chat(agent);
                    let (last, time) = chat.last_said();
                    (last, time, chat.writing())
                };
                json!({ "id": agent.id, "name": agent.name, "colour": agent.colour, "job": agent.job, "can": agent.can, "skills": agent.skills, "links": agent.links, "last": last, "time": time, "writing": writing })
            })
            .collect();
        Ok(Value::Array(rows).to_string())
    }

    /// Adds an agent to the team, and returns its id. `can` is what it may look at: any of mail, calendar, tasks, notes,
    /// web and laptop, all read-only. `skills` are `SKILL.md` folders whose text is part of its instructions. `links` are
    /// the agents it may hand work to.
    #[allow(clippy::too_many_arguments)] // The call's arguments are the agent's fields.
    async fn create_agent(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        name: String,
        job: String,
        soul: String,
        can: Vec<String>,
        skills: Vec<String>,
        links: Vec<String>,
    ) -> Result<String, Error> {
        let agent = agents::create(&name, &job, &soul, &can, &skills, &links).map_err(Error::Failed)?;
        self.shared.events.record(chat::now(), "created", &agent.id, "");
        Self::agents_changed(&emitter).await?;
        Ok(agent.id)
    }

    /// Removes an agent, its conversation and its memory.
    async fn delete_agent(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String) -> Result<(), Error> {
        agents::delete(&id).map_err(Error::Failed)?;
        {
            let mut core = self.shared.core();
            core.chats.remove(&id);
            core.pending.retain(|_, pending| pending.agent != id);
        }
        self.shared.events.record(chat::now(), "deleted", &id, "");
        Self::agents_changed(&emitter).await?;
        Ok(())
    }

    /// Changes an agent: `patch` is JSON with any of `job`, `soul`, `can`, `skills` and `links`.
    async fn set_agent(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String, patch: String) -> Result<(), Error> {
        let patch: agents::Patch = serde_json::from_str(&patch).map_err(|err| Error::Failed(format!("not a change to an agent: {err}")))?;
        agents::update(&id, &patch).map_err(Error::Failed)?;
        self.shared.events.record(chat::now(), "changed", &id, "");
        Self::agents_changed(&emitter).await?;
        Ok(())
    }

    /// An agent's conversation, as a JSON array of entries: `{"id","time","kind":"you","text"}` and
    /// `{"id","time","kind":"answer","answer","text","writing","error"}`.
    async fn history(&self, #[zbus(connection)] connection: &zbus::Connection, id: String) -> Result<String, Error> {
        let agent = find(&id)?;
        if agent.id == KITH {
            return ai(connection).await?.history().await.map_err(engine);
        }
        let mut core = self.shared.core();
        serde_json::to_string(core.chat(&agent).entries()).map_err(|err| Error::Failed(err.to_string()))
    }

    /// Says something to an agent. The answer is kept in its conversation as an entry that grows as it is written, and
    /// comes as `AnswerPiece` signals too. Returns the answer's id and its entry.
    async fn say(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String, text: String) -> Result<(u32, u64), Error> {
        let agent = find(&id)?;
        let text = text.trim().to_owned();
        if text.is_empty() {
            return failed("there is nothing to say");
        }
        let proxy = ai(connection).await?;
        self.shared.events.record(chat::now(), "said", &agent.id, &text);
        if agent.id == KITH {
            let routed = proxy.chat(&text).await.map_err(engine)?;
            let (question, files) = kith_question(&routed, &text).map_err(Error::Failed)?;
            let files: Vec<&str> = files.iter().map(String::as_str).collect();
            let (answer, entry, _) = proxy.chat_answer(&question, &files).await.map_err(engine)?;
            return Ok((answer, entry));
        }
        // Asked to remember something: code proposes the fact for you to save, and no model is asked, so nothing is
        // claimed that wasn't done.
        if let Some(fact) = remember::fact(&text) {
            let entry = {
                let mut core = self.shared.core();
                let chat = core.chat(&agent);
                if chat.writing() {
                    return failed(format!("{} is still answering", agent.name));
                }
                chat.push(What::You { text: text.clone() });
                chat.push(What::Answer { answer: 0, text: "I can keep that in my memory if you say so.".to_owned(), writing: false, error: String::new() });
                let entry = chat.push(What::Fact { text: fact, state: FactState::Proposed });
                chat.save();
                entry
            };
            Self::agent_changed(&emitter, &agent.id).await?;
            return Ok((0, entry));
        }
        // Your words and an answer begun go in the conversation first, so the roster shows it is being written.
        let (entry, conversation) = {
            let mut core = self.shared.core();
            let chat = core.chat(&agent);
            if chat.writing() {
                return failed(format!("{} is still answering", agent.name));
            }
            let conversation = chat.transcript();
            chat.push(What::You { text: text.clone() });
            let entry = chat.push(What::Answer { answer: 0, text: String::new(), writing: true, error: String::new() });
            chat.save();
            (entry, conversation)
        };
        Self::agent_changed(&emitter, &agent.id).await?;
        let started = async {
            // What the agent may look at that the question is about, read now by `autark-ai` and only from `can`.
            let can: Vec<&str> = agent.can.iter().map(String::as_str).collect();
            let seen = match proxy.look(&text, &can).await {
                Ok(seen) => seen,
                Err(err) => {
                    eprintln!("agentd: looking for {}: {}", agent.name, ai::said(&err));
                    String::new()
                }
            };
            let facts = agents::memory_path(&agent.id).map(|path| memory::load(&path)).unwrap_or_default();
            let skills: Vec<(String, String)> = agent.skills.iter().filter_map(|name| agents::skill_text(name).map(|text| (name.clone(), text))).collect();
            proxy.complete(&prompt::system(&agent, &skills, &facts), &prompt::context(&conversation, &seen), &text).await
        }
        .await;
        match started {
            Ok(answer) => {
                // An answer that failed at once has ended by now.
                let ended = self.shared.core().register(answer, &agent, entry);
                announce(&self.shared, &emitter, ended).await;
                Ok((answer, entry))
            }
            Err(err) => {
                let why = ai::said(&err);
                if let Some(What::Answer { writing, error, .. }) = self.shared.core().chat(&agent).get_mut(entry) {
                    *writing = false;
                    *error = why.clone();
                }
                self.shared.core().chat(&agent).save();
                self.shared.events.record(chat::now(), "failed", &agent.id, &why);
                Self::agent_changed(&emitter, &agent.id).await?;
                Err(Error::Failed(why))
            }
        }
    }

    /// Clears an agent's conversation.
    async fn clear(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String) -> Result<(), Error> {
        let agent = find(&id)?;
        if agent.id == KITH {
            ai(connection).await?.clear_history().await.map_err(engine)?;
        } else {
            let mut core = self.shared.core();
            core.pending.retain(|_, pending| pending.agent != id);
            let chat = core.chat(&agent);
            chat.clear();
            chat.save();
        }
        Self::agent_changed(&emitter, &agent.id).await?;
        Ok(())
    }

    /// Stops an answer still being written; it ends with what was written so far and no error.
    async fn stop(&self, #[zbus(connection)] connection: &zbus::Connection, answer: u32) -> Result<(), Error> {
        ai(connection).await?.stop(answer).await.map_err(engine)
    }

    /// An agent's memory as JSON: `[{"text","added"}]`, at most 12 facts and 2,000 characters in all.
    async fn memory(&self, id: String) -> Result<String, Error> {
        let path = memory_of(&id)?;
        serde_json::to_string(&memory::load(&path)).map_err(|err| Error::Failed(err.to_string()))
    }

    /// Adds a fact to an agent's memory, or says why it can't: the memory is full, or the fact is too long.
    async fn remember(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String, text: String) -> Result<(), Error> {
        memory::add(&memory_of(&id)?, &text, chat::now()).map_err(Error::Failed)?;
        self.shared.events.record(chat::now(), "remembered", &id, "");
        Self::agent_changed(&emitter, &id).await?;
        Ok(())
    }

    /// Forgets the `n`th fact of an agent's memory, counting from 0.
    async fn forget(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String, n: u32) -> Result<(), Error> {
        memory::forget(&memory_of(&id)?, n as usize).map_err(Error::Failed)?;
        self.shared.events.record(chat::now(), "forgot", &id, "");
        Self::agent_changed(&emitter, &id).await?;
        Ok(())
    }

    /// Answers a fact an agent proposed (the entry of a `fact` in its conversation): saves it to its memory, or not. A fact
    /// that can't be saved, because the memory is full, says why and stays to be answered again.
    async fn answer_fact(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String, entry: u64, save: bool) -> Result<(), Error> {
        let agent = find(&id)?;
        let path = memory_of(&id)?;
        {
            let mut core = self.shared.core();
            let chat = core.chat(&agent);
            let Some((text, state)) = chat.fact_mut(entry) else { return failed("there is no such fact") };
            if *state != FactState::Proposed {
                return failed("that fact has been answered already");
            }
            if save {
                memory::add(&path, text, chat::now()).map_err(Error::Failed)?;
            }
            *state = if save { FactState::Saved } else { FactState::Dismissed };
            chat.save();
        }
        self.shared.events.record(chat::now(), if save { "remembered" } else { "not-remembered" }, &id, "");
        Self::agent_changed(&emitter, &id).await?;
        Ok(())
    }

    /// What happened since `since` (seconds since the epoch), as a JSON array of `{"time","kind","agent","text"}`.
    async fn events(&self, since: u64) -> String {
        self.shared.events.since(since)
    }

    #[zbus(signal)]
    async fn agent_changed(emitter: &SignalEmitter<'_>, id: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn agents_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// A piece of answer `answer`, which starts at byte `offset` of the answer's text.
    #[zbus(signal)]
    async fn answer_piece(emitter: &SignalEmitter<'_>, answer: u32, offset: u32, text: &str) -> zbus::Result<()>;
}

fn memory_of(id: &str) -> Result<std::path::PathBuf, Error> {
    if id == KITH {
        return failed("Kith's memory is kept in the launcher's panel");
    }
    find(id)?;
    agents::memory_path(id).ok_or_else(|| Error::Failed(format!("there is no agent {id}")))
}

/// Follows `autark-ai` while the daemon runs: writes the answers it streams into the agents' conversations, passes the
/// pieces on to clients, and gives up on an answer that has gone quiet. Starts again when `autark-ai` has gone away.
pub async fn follow(shared: Arc<Shared>, connection: zbus::Connection) {
    let emitter = match SignalEmitter::new(&connection, PATH) {
        Ok(emitter) => emitter,
        Err(err) => {
            eprintln!("agentd: {err}");
            return;
        }
    };
    let watch = async {
        loop {
            async_io::Timer::after(Duration::from_secs(30)).await;
            let idle: Vec<u32> = shared.core().pending.iter().filter(|(_, pending)| pending.heard.elapsed() > IDLE).map(|(id, _)| *id).collect();
            for answer in idle {
                if let Ok(proxy) = AiProxy::new(&connection).await {
                    let _ = proxy.stop(answer).await;
                }
                let done = shared.core().finish(answer, "The model didn't answer in time.");
                announce(&shared, &emitter, done).await;
            }
        }
    };
    let listen = async {
        loop {
            let result = async {
                let proxy = AiProxy::new(&connection).await?;
                let pieces = proxy.receive_answer_piece().await?.filter_map(|signal| std::future::ready(signal.args().ok().map(|args| Told::Piece(args.id, args.offset, args.text))));
                let done = proxy.receive_answer_done().await?.filter_map(|signal| std::future::ready(signal.args().ok().map(|args| Told::Done(args.id, args.error))));
                let kith = proxy.receive_history_changed().await?.map(|_| Told::Kith);
                let mut heard = futures_util::stream::select(futures_util::stream::select(pieces, done), kith);
                while let Some(heard) = heard.next().await {
                    match heard {
                        // Passed on at once, whoever started it: the roster follows Kith's answers too.
                        Told::Piece(id, offset, text) => {
                            if let Err(err) = Agents::answer_piece(&emitter, id, offset, &text).await {
                                eprintln!("agentd: sending a piece: {err}");
                            }
                            shared.core().hear(id, Heard::Piece(text));
                        }
                        Told::Kith => announce(&shared, &emitter, Some((KITH.to_owned(), "", String::new()))).await,
                        Told::Done(id, error) => {
                            let ended = shared.core().hear(id, Heard::Done(error));
                            announce(&shared, &emitter, ended).await;
                        }
                    }
                }
                zbus::Result::Ok(())
            };
            if let Err(err) = result.await {
                eprintln!("agentd: following autark-ai: {err}");
            }
            async_io::Timer::after(Duration::from_secs(3)).await;
        }
    };
    futures_util::future::join(watch, listen).await;
}

async fn announce(shared: &Shared, emitter: &SignalEmitter<'_>, ended: Option<(String, &'static str, String)>) {
    if let Some((agent, kind, said)) = ended {
        if !kind.is_empty() {
            shared.events.record(chat::now(), kind, &agent, &said);
        }
        if let Err(err) = Agents::agent_changed(emitter, &agent).await {
            eprintln!("agentd: sending a change: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str) -> Agent {
        Agent { id: id.into(), name: id.to_uppercase(), colour: 0, job: String::new(), soul: String::new(), can: vec![], skills: vec![], links: vec![] }
    }

    fn core_with(agent: &Agent) -> (Core, u64) {
        let mut core = Core::default();
        core.chats.insert(agent.id.clone(), Chat::in_memory(&agent.name));
        let entry = core.chat(agent).push(What::Answer { answer: 0, text: String::new(), writing: true, error: String::new() });
        (core, entry)
    }

    fn text_of(core: &mut Core, agent: &Agent) -> (String, bool, String) {
        match &core.chat(agent).entries()[0].what {
            What::Answer { text, writing, error, .. } => (text.clone(), *writing, error.clone()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pieces_grow_the_answer_and_done_ends_it() {
        let a = agent("piper");
        let (mut core, entry) = core_with(&a);
        assert!(core.register(7, &a, entry).is_none());
        assert!(core.hear(7, Heard::Piece("Hel".into())).is_none());
        core.hear(7, Heard::Piece("lo.".into()));
        let ended = core.hear(7, Heard::Done(String::new())).unwrap();
        assert_eq!((ended.0.as_str(), ended.1, ended.2.as_str()), ("piper", "answered", "Hello."));
        assert_eq!(text_of(&mut core, &a), ("Hello.".to_owned(), false, String::new()));
        assert!(core.pending.is_empty());
    }

    #[test]
    fn words_heard_before_the_answer_is_known_are_matched_after() {
        let a = agent("piper");
        let (mut core, entry) = core_with(&a);
        core.hear(9, Heard::Piece("Early".into()));
        core.hear(4, Heard::Piece("not ours".into()));
        core.hear(9, Heard::Done(String::new()));
        assert_eq!(core.register(9, &a, entry).map(|ended| ended.1), Some("answered"));
        assert_eq!(text_of(&mut core, &a), ("Early".to_owned(), false, String::new()));
        // What belongs to others stays to be matched.
        assert_eq!(core.early.len(), 1);
    }

    #[test]
    fn an_answer_that_fails_says_why_and_logs_a_failure() {
        let a = agent("piper");
        let (mut core, entry) = core_with(&a);
        core.register(3, &a, entry);
        let ended = core.hear(3, Heard::Done("Ollama isn't running".into())).unwrap();
        assert_eq!(ended.1, "failed");
        assert_eq!(text_of(&mut core, &a), (String::new(), false, "Ollama isn't running".to_owned()));
    }

    #[test]
    fn an_empty_answer_without_an_error_is_a_failure_too() {
        let a = agent("piper");
        let (mut core, entry) = core_with(&a);
        core.register(3, &a, entry);
        assert_eq!(core.hear(3, Heard::Done(String::new())).unwrap().1, "failed");
        assert_eq!(text_of(&mut core, &a).2, "No answer came back.");
    }

    #[test]
    fn what_kith_is_asked_comes_from_how_it_routed() {
        assert_eq!(kith_question(r#"{"question":"Why?","files":["a.md"],"actions":[]}"#, "why"), Ok(("Why?".to_owned(), vec!["a.md".to_owned()])));
        assert_eq!(kith_question(r#"{"question":"","actions":[]}"#, "hi"), Ok(("hi".to_owned(), vec![])));
        assert!(kith_question(r#"{"question":"","actions":[{"action":"mute"}]}"#, "mute").unwrap_err().contains("launcher"));
        assert!(kith_question("nonsense", "x").is_err());
    }

    #[test]
    fn kiths_last_words_skip_cards() {
        let history = r#"[{"kind":"you","text":"hi","time":5},{"kind":"actions","time":6},{"kind":"answer","text":"","writing":true,"time":7}]"#;
        assert_eq!(kith_last(history), ("hi".to_owned(), 5, true));
        assert_eq!(kith_last("[]"), (String::new(), 0, false));
    }
}
