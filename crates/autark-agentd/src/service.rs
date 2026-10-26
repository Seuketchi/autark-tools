//! `org.autark.Agents1`: the team's API. It decides which agent answers, what the agent may look at and what it is told;
//! `autark-ai` reads what it is allowed to read and runs the model. A client (the roster, later the launcher or the bar)
//! talks to this and nothing else.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;

use crate::agents::{self, Agent, KITH};
use crate::ai::{self, AiProxy};
use crate::chat::{self, Chat, FactState, What};
use crate::events::Events;
use crate::mailbox;
use crate::memory;
use crate::notes;
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

/// Where an answer comes from in a chain of handoffs.
#[derive(Clone, Default)]
struct Meta {
    /// How many handoffs led to it: 0 for an answer to you.
    depth: u8,
    /// The agents in the chain so far, itself last.
    chain: Vec<String>,
    /// The agent that asked for it, which gets the answer back.
    origin: Option<String>,
}

/// An answer being written by an agent here.
struct Pending {
    agent: String,
    heard: Instant,
    meta: Meta,
}

/// Work handed to an agent, put in its conversation and waiting to be started.
struct Delivery {
    from: Agent,
    to: Agent,
    /// The answer begun in the receiver's conversation.
    entry: u64,
    brief: String,
    /// What the receiver had said before, for its context.
    conversation: String,
    meta: Meta,
}

/// How an answer ended, and what follows from it.
pub struct Ended {
    agent: String,
    /// What to log for the answer: `answered` or `failed`; empty when nothing is to be logged.
    kind: &'static str,
    said: String,
    /// The other agents whose conversations changed: the one given work, the one that asked.
    changed: Vec<String>,
    /// Further events: kind, agent and what.
    logged: Vec<(&'static str, String, String)>,
    delivery: Option<Delivery>,
}

impl Ended {
    fn of(agent: &str, kind: &'static str, said: String) -> Self {
        Self { agent: agent.to_owned(), kind, said, changed: Vec::new(), logged: Vec::new(), delivery: None }
    }
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
    fn register(&mut self, answer: u32, agent: &Agent, entry: u64, meta: Meta, team: &[Agent]) -> Option<Ended> {
        if let Some(What::Answer { answer: id, .. }) = self.chat(agent).get_mut(entry) {
            *id = answer;
        }
        self.pending.insert(answer, Pending { agent: agent.id.clone(), heard: Instant::now(), meta });
        let mut finished = None;
        let early = std::mem::take(&mut self.early);
        for (id, heard) in early {
            if id != answer {
                self.early.push_back((id, heard));
                continue;
            }
            match heard {
                Heard::Piece(text) => self.append(answer, &text),
                Heard::Done(error) => finished = self.finish(answer, &error, team),
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

    /// Ends an answer, with `error` empty when it went well. A handoff its last line asks for is taken out of it and made
    /// (or refused, with the reason, in the asker's conversation), and an answer to work handed over goes back to the agent
    /// that asked.
    fn finish(&mut self, answer: u32, error: &str, team: &[Agent]) -> Option<Ended> {
        let pending = self.pending.remove(&answer)?;
        let chat = self.chats.get_mut(&pending.agent)?;
        let mut said = String::new();
        let mut well = false;
        let mut asked = None;
        if let Some(What::Answer { text, writing, error: why, .. }) = chat.answer_mut(answer) {
            *writing = false;
            if !error.is_empty() {
                *why = error.to_owned();
            } else if text.is_empty() {
                *why = "No answer came back.".to_owned();
            }
            well = why.is_empty();
            if well {
                // A handoff the last line asks for is not part of what is said.
                let (visible, handoff) = mailbox::split(text);
                if handoff.is_some() {
                    *text = visible;
                }
                asked = handoff;
            }
            said = if well { text.clone() } else { why.clone() };
        }
        let mut ended = Ended::of(&pending.agent, if well { "answered" } else { "failed" }, String::new());
        let from = team.iter().find(|agent| agent.id == pending.agent).cloned();
        if let (Some(asked), Some(from)) = (asked, &from) {
            self.hand_on(from, asked, &pending.meta, team, &mut ended);
        }
        if let Some(chat) = self.chats.get_mut(&pending.agent) {
            chat.save();
        }
        if let (Some(origin), Some(from)) = (&pending.meta.origin, &from)
            && let Some(asker) = team.iter().find(|agent| &agent.id == origin)
        {
            let text = if well { said.clone() } else { format!("(couldn't answer: {said})") };
            let chat = self.chat(asker);
            chat.push(What::From { from: from.id.clone(), name: from.name.clone(), text, reply: true });
            chat.save();
            ended.changed.push(asker.id.clone());
            ended.logged.push(("replied", from.id.clone(), format!("to {}", asker.id)));
        }
        ended.said = said;
        Some(ended)
    }

    /// Makes the handoff `from` asked for, or records why not: its entry in the asker's conversation, and, if it goes,
    /// a message and an answer begun in the receiver's.
    fn hand_on(&mut self, from: &Agent, asked: mailbox::Asked, meta: &Meta, team: &[Agent], ended: &mut Ended) {
        let target = mailbox::resolve(&asked.to, from, team).and_then(|to| {
            let busy = self.chat(&to).writing();
            mailbox::check(from, &to, meta.depth, &meta.chain, busy).map(|()| to)
        });
        match target {
            Ok(to) => {
                self.chat(from).push(What::Handoff { to: to.id.clone(), name: to.name.clone(), brief: asked.brief.clone(), refused: String::new() });
                let conversation = self.chat(&to).transcript();
                let chat = self.chat(&to);
                chat.push(What::From { from: from.id.clone(), name: from.name.clone(), text: asked.brief.clone(), reply: false });
                let entry = chat.push(What::Answer { answer: 0, text: String::new(), writing: true, error: String::new() });
                chat.save();
                let mut chain = meta.chain.clone();
                chain.push(to.id.clone());
                ended.changed.push(to.id.clone());
                ended.logged.push(("handoff", from.id.clone(), format!("to {}: {}", to.id, asked.brief)));
                ended.delivery = Some(Delivery { from: from.clone(), to, entry, brief: asked.brief, conversation, meta: Meta { depth: meta.depth + 1, chain, origin: Some(from.id.clone()) } });
            }
            Err(why) => {
                self.chat(from).push(What::Handoff { to: asked.to.clone(), name: asked.to.clone(), brief: asked.brief, refused: why.clone() });
                ended.logged.push(("handoff-refused", from.id.clone(), why));
            }
        }
    }

    /// An answer that couldn't be started: its entry says why.
    fn fail(&mut self, agent: &Agent, entry: u64, why: &str) {
        let chat = self.chat(agent);
        if let Some(What::Answer { writing, error, .. }) = chat.get_mut(entry) {
            *writing = false;
            *error = why.to_owned();
        }
        chat.save();
    }

    fn hear(&mut self, answer: u32, heard: Heard, team: &[Agent]) -> Option<Ended> {
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
            Heard::Done(error) => self.finish(answer, &error, team),
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
        // Asked to remember something or to add to a note: code proposes it for you to confirm, and no model is asked, so
        // nothing is claimed that wasn't done.
        let proposal = if let Some(fact) = remember::fact(&text) {
            Some(("I can keep that in my memory if you say so.", Some(What::Fact { text: fact, state: FactState::Proposed })))
        } else {
            notes::parse(&text).map(|add| {
                if agent.can.iter().any(|can| can == "notes") {
                    ("I can add that to your note if you say so.", Some(What::Add { note: add.note, items: add.items, state: FactState::Proposed }))
                } else {
                    ("I'm not set up to look at your notes, so I can't add to them.", None)
                }
            })
        };
        if let Some((said, card)) = proposal {
            let entry = {
                let mut core = self.shared.core();
                let chat = core.chat(&agent);
                if chat.writing() {
                    return failed(format!("{} is still answering", agent.name));
                }
                chat.push(What::You { text: text.clone() });
                let entry = chat.push(What::Answer { answer: 0, text: said.to_owned(), writing: false, error: String::new() });
                let entry = card.map_or(entry, |card| chat.push(card));
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
        let meta = Meta { depth: 0, chain: vec![agent.id.clone()], origin: None };
        match start_answer(&self.shared, connection, &agent, entry, &text, &conversation, None, meta).await {
            Ok((answer, ended)) => {
                // An answer that failed at once has ended by now.
                settle(self.shared.clone(), connection.clone(), emitter.into_owned(), ended).await;
                Ok((answer, entry))
            }
            Err(why) => {
                self.shared.core().fail(&agent, entry, &why);
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

    /// Answers an addition to a note an agent proposed (the entry of an `add` in its conversation): adds the lines at the
    /// end of the note, or not. When the note can't be written to it says why and the proposal stays to be answered again.
    async fn answer_add(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, id: String, entry: u64, add: bool) -> Result<(), Error> {
        let agent = find(&id)?;
        let note = {
            let mut core = self.shared.core();
            let chat = core.chat(&agent);
            let Some((note, items, state)) = chat.add_mut(entry) else { return failed("there is no such addition") };
            if *state != FactState::Proposed {
                return failed("that has been answered already");
            }
            if add {
                if !agent.can.iter().any(|can| can == "notes") {
                    return failed(format!("{} isn't set up to look at your notes", agent.name));
                }
                notes::append(&notes::vault(), &notes::Add { note: note.to_owned(), items: items.to_vec() }).map_err(Error::Failed)?;
            }
            let note = note.to_owned();
            *state = if add { FactState::Saved } else { FactState::Dismissed };
            chat.save();
            note
        };
        self.shared.events.record(chat::now(), if add { "noted" } else { "not-noted" }, &id, &note);
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
                let team = agents::all();
                let ended = shared.core().finish(answer, "The model didn't answer in time.", &team);
                settle(shared.clone(), connection.clone(), emitter.clone(), ended).await;
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
                            shared.core().hear(id, Heard::Piece(text), &[]);
                        }
                        Told::Kith => settle(shared.clone(), connection.clone(), emitter.clone(), Some(Ended::of(KITH, "", String::new()))).await,
                        Told::Done(id, error) => {
                            let team = agents::all();
                            let ended = shared.core().hear(id, Heard::Done(error), &team);
                            settle(shared.clone(), connection.clone(), emitter.clone(), ended).await;
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

/// Asks `autark-ai` for an agent's answer: what it may look at that the question is about, then the answer itself under the
/// agent's instructions. Registers the answer, and returns its id and how it ended if that was already heard. `from` is who
/// handed the work over, when it was not you.
#[allow(clippy::too_many_arguments)] // What an answer needs, from where it is asked.
async fn start_answer(
    shared: &Shared,
    connection: &zbus::Connection,
    agent: &Agent,
    entry: u64,
    prompt: &str,
    conversation: &str,
    from: Option<&str>,
    meta: Meta,
) -> Result<(u32, Option<Ended>), String> {
    let proxy = AiProxy::new(connection).await.map_err(|err| ai::said(&err))?;
    // What the agent may look at that the question is about, read now by `autark-ai` and only from `can`.
    let can: Vec<&str> = agent.can.iter().map(String::as_str).collect();
    // Work another agent handed over says "the user's notes" where you would say "my notes", which is what is looked for.
    let asked = if from.is_some() { as_mine(prompt) } else { prompt.to_owned() };
    let seen = match proxy.look(&asked, &can).await {
        Ok(seen) => seen,
        Err(err) => {
            eprintln!("agentd: looking for {}: {}", agent.name, ai::said(&err));
            String::new()
        }
    };
    let facts = agents::memory_path(&agent.id).map(|path| memory::load(&path)).unwrap_or_default();
    let skills: Vec<(String, String)> = agent.skills.iter().filter_map(|name| agents::skill_text(name).map(|text| (name.clone(), text))).collect();
    let team = agents::all();
    let links: Vec<Agent> = agent.links.iter().filter_map(|link| team.iter().find(|other| &other.id == link).cloned()).collect();
    let mut context = prompt::context(conversation, &seen);
    if let Some(name) = from {
        context.push_str(&format!("\n\nThis message is from the agent {name}, not from the user, who can see it. Answer it for them, briefly."));
    }
    let answer = proxy.complete(&prompt::system(agent, &skills, &facts, &links), context.trim_start(), prompt).await.map_err(|err| ai::said(&err))?;
    let ended = shared.core().register(answer, agent, entry, meta, &team);
    Ok((answer, ended))
}

/// A request from another agent as you would have said it: "the user's notes", "your notes" and "their notes" as "my
/// notes", which is how what to look at is told.
fn as_mine(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let core = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’');
        let (lead, tail) = (&word[..word.find(core).unwrap_or(0)], &word[word.find(core).unwrap_or(0) + core.len()..]);
        let mine = match core.to_lowercase().as_str() {
            "user's" | "user’s" | "your" | "their" => Some("my"),
            "user" if out.last().is_some_and(|before| before.eq_ignore_ascii_case("the")) => Some("I"),
            _ => None,
        };
        match mine {
            Some(mine) => {
                if out.last().is_some_and(|before| before.eq_ignore_ascii_case("the")) {
                    out.pop();
                }
                out.push(format!("{lead}{mine}{tail}"));
            }
            None => out.push(word.to_owned()),
        }
    }
    out.join(" ")
}

/// What follows an answer's end: it is logged, clients are told which conversations changed, and work it handed on is
/// started.
fn settle(shared: Arc<Shared>, connection: zbus::Connection, emitter: SignalEmitter<'static>, ended: Option<Ended>) -> BoxFuture<'static, ()> {
    Box::pin(async move {
        let Some(ended) = ended else { return };
        if !ended.kind.is_empty() {
            shared.events.record(chat::now(), ended.kind, &ended.agent, &ended.said);
        }
        for (kind, agent, what) in &ended.logged {
            shared.events.record(chat::now(), kind, agent, what);
        }
        for id in std::iter::once(&ended.agent).chain(&ended.changed) {
            if let Err(err) = Agents::agent_changed(&emitter, id).await {
                eprintln!("agentd: sending a change: {err}");
            }
        }
        if let Some(delivery) = ended.delivery {
            let Delivery { from, to, entry, brief, conversation, meta } = delivery;
            match start_answer(&shared, &connection, &to, entry, &brief, &conversation, Some(&from.name), meta).await {
                Ok((_, ended)) => settle(shared, connection, emitter, ended).await,
                Err(why) => {
                    shared.core().fail(&to, entry, &why);
                    shared.events.record(chat::now(), "failed", &to.id, &why);
                    if let Err(err) = Agents::agent_changed(&emitter, &to.id).await {
                        eprintln!("agentd: sending a change: {err}");
                    }
                }
            }
        }
    })
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
        assert!(core.register(7, &a, entry, Meta::default(), &[]).is_none());
        assert!(core.hear(7, Heard::Piece("Hel".into()), &[]).is_none());
        core.hear(7, Heard::Piece("lo.".into()), &[]);
        let ended = core.hear(7, Heard::Done(String::new()), &[]).unwrap();
        assert_eq!((ended.agent.as_str(), ended.kind, ended.said.as_str()), ("piper", "answered", "Hello."));
        assert_eq!(text_of(&mut core, &a), ("Hello.".to_owned(), false, String::new()));
        assert!(core.pending.is_empty());
    }

    #[test]
    fn words_heard_before_the_answer_is_known_are_matched_after() {
        let a = agent("piper");
        let (mut core, entry) = core_with(&a);
        core.hear(9, Heard::Piece("Early".into()), &[]);
        core.hear(4, Heard::Piece("not ours".into()), &[]);
        core.hear(9, Heard::Done(String::new()), &[]);
        assert_eq!(core.register(9, &a, entry, Meta::default(), &[]).map(|ended| ended.kind), Some("answered"));
        assert_eq!(text_of(&mut core, &a), ("Early".to_owned(), false, String::new()));
        // What belongs to others stays to be matched.
        assert_eq!(core.early.len(), 1);
    }

    #[test]
    fn an_answer_that_fails_says_why_and_logs_a_failure() {
        let a = agent("piper");
        let (mut core, entry) = core_with(&a);
        core.register(3, &a, entry, Meta::default(), &[]);
        let ended = core.hear(3, Heard::Done("Ollama isn't running".into()), &[]).unwrap();
        assert_eq!(ended.kind, "failed");
        assert_eq!(text_of(&mut core, &a), (String::new(), false, "Ollama isn't running".to_owned()));
    }

    #[test]
    fn an_empty_answer_without_an_error_is_a_failure_too() {
        let a = agent("piper");
        let (mut core, entry) = core_with(&a);
        core.register(3, &a, entry, Meta::default(), &[]);
        assert_eq!(core.hear(3, Heard::Done(String::new()), &[]).unwrap().kind, "failed");
        assert_eq!(text_of(&mut core, &a).2, "No answer came back.");
    }

    #[test]
    fn work_handed_over_is_looked_up_as_if_you_had_asked_it() {
        assert_eq!(as_mine("What do the user's notes say about the budget?"), "What do my notes say about the budget?");
        assert_eq!(as_mine("What do your notes say about the budget?"), "What do my notes say about the budget?");
        assert_eq!(as_mine("what has the user written in their notes?"), "what has I written in my notes?");
        assert_eq!(as_mine("Find the User’s calendar for Friday"), "Find my calendar for Friday");
        assert_eq!(as_mine("Plain request"), "Plain request");
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

    fn linked(id: &str, links: &[&str]) -> Agent {
        Agent { links: links.iter().map(|link| (*link).to_owned()).collect(), ..agent(id) }
    }

    /// A conversation for each of `team`, with `who`'s answer under way, as `said`.
    fn handoff_core(team: &[Agent], who: &str, said: &str, meta: Meta) -> Core {
        let mut core = Core::default();
        for agent in team {
            core.chats.insert(agent.id.clone(), Chat::in_memory(&agent.name));
        }
        let agent = team.iter().find(|agent| agent.id == who).unwrap();
        let entry = core.chat(agent).push(What::Answer { answer: 0, text: said.to_owned(), writing: true, error: String::new() });
        core.register(5, agent, entry, meta, team);
        core
    }

    fn in_chain(ids: &[&str], depth: u8) -> Meta {
        Meta { depth, chain: ids.iter().map(|id| (*id).to_owned()).collect(), origin: None }
    }

    #[test]
    fn a_handoff_is_taken_out_of_the_answer_and_given_to_the_other_agent() {
        let team = [linked("atlas", &["raven"]), linked("raven", &[])];
        let mut core = handoff_core(&team, "atlas", "Rain is likely.\nHANDOFF raven: forecast for Cebu tomorrow?", in_chain(&["atlas"], 0));
        let ended = core.hear(5, Heard::Done(String::new()), &team).unwrap();
        assert_eq!(ended.said, "Rain is likely.");
        let delivery = ended.delivery.unwrap();
        assert_eq!((delivery.to.id.as_str(), delivery.brief.as_str(), delivery.meta.depth), ("raven", "forecast for Cebu tomorrow?", 1));
        assert_eq!(delivery.meta.chain, ["atlas", "raven"]);
        assert_eq!(delivery.meta.origin.as_deref(), Some("atlas"));
        assert_eq!(ended.changed, ["raven"]);
        // Both conversations show it, and the receiver's answer is begun, written by the daemon in the sender's name.
        let atlas = core.chat(&team[0]).entries().to_vec();
        assert!(matches!(&atlas[0].what, What::Answer { text, .. } if text == "Rain is likely."));
        assert!(matches!(&atlas[1].what, What::Handoff { to, refused, .. } if to == "raven" && refused.is_empty()));
        let raven = core.chat(&team[1]).entries().to_vec();
        assert!(matches!(&raven[0].what, What::From { from, name, reply: false, .. } if from == "atlas" && name == "ATLAS"));
        assert!(matches!(&raven[1].what, What::Answer { writing: true, .. }));
        assert_eq!(delivery.entry, raven[1].id);
    }

    #[test]
    fn a_handoff_to_an_agent_that_isnt_linked_is_refused_and_says_so() {
        let team = [linked("atlas", &[]), linked("raven", &[])];
        let mut core = handoff_core(&team, "atlas", "HANDOFF raven: look it up", in_chain(&["atlas"], 0));
        let ended = core.hear(5, Heard::Done(String::new()), &team).unwrap();
        assert!(ended.delivery.is_none() && ended.changed.is_empty());
        assert!(matches!(&core.chat(&team[0]).entries().last().unwrap().what, What::Handoff { refused, .. } if refused.contains("isn't linked")));
        assert!(core.chat(&team[1]).entries().is_empty());
        assert_eq!(ended.logged[0].0, "handoff-refused");
    }

    #[test]
    fn a_chain_three_deep_cannot_hand_on_and_one_never_returns_to_an_agent_in_it() {
        let team = [linked("a", &["b"]), linked("b", &["a", "c"]), linked("c", &[])];
        let mut deep = handoff_core(&team, "b", "HANDOFF c: x", in_chain(&["a", "b"], 3));
        assert!(deep.hear(5, Heard::Done(String::new()), &team).unwrap().delivery.is_none());
        assert!(matches!(&deep.chat(&team[1]).entries().last().unwrap().what, What::Handoff { refused, .. } if refused.contains("3 deep")));
        let mut back = handoff_core(&team, "b", "HANDOFF a: x", in_chain(&["a", "b"], 1));
        assert!(back.hear(5, Heard::Done(String::new()), &team).unwrap().delivery.is_none());
        assert!(matches!(&back.chat(&team[1]).entries().last().unwrap().what, What::Handoff { refused, .. } if refused.contains("already in this chain")));
    }

    #[test]
    fn work_isnt_handed_to_an_agent_that_is_busy_answering() {
        let team = [linked("atlas", &["raven"]), linked("raven", &[])];
        let mut core = handoff_core(&team, "atlas", "HANDOFF raven: x", in_chain(&["atlas"], 0));
        core.chat(&team[1]).push(What::Answer { answer: 9, text: String::new(), writing: true, error: String::new() });
        assert!(core.hear(5, Heard::Done(String::new()), &team).unwrap().delivery.is_none());
        assert!(matches!(&core.chat(&team[0]).entries().last().unwrap().what, What::Handoff { refused, .. } if refused.contains("busy")));
    }

    #[test]
    fn an_answer_to_handed_over_work_goes_back_to_the_agent_that_asked() {
        let team = [linked("atlas", &["raven"]), linked("raven", &[])];
        let meta = Meta { depth: 1, chain: vec!["atlas".into(), "raven".into()], origin: Some("atlas".into()) };
        let mut core = handoff_core(&team, "raven", "", meta.clone());
        core.hear(5, Heard::Piece("Rain, 28 degrees.".into()), &team);
        let ended = core.hear(5, Heard::Done(String::new()), &team).unwrap();
        assert_eq!(ended.changed, ["atlas"]);
        assert!(matches!(&core.chat(&team[0]).entries()[0].what, What::From { from, text, reply: true, .. } if from == "raven" && text == "Rain, 28 degrees."));
        // A failure goes back too, so the asker's conversation says it didn't work.
        let mut failed = handoff_core(&team, "raven", "", meta);
        failed.hear(5, Heard::Done("Ollama isn't running".into()), &team);
        assert!(matches!(&failed.chat(&team[0]).entries()[0].what, What::From { text, reply: true, .. } if text.contains("Ollama isn't running")));
    }

    #[test]
    fn an_answer_that_hands_on_after_being_handed_work_does_both() {
        let team = [linked("atlas", &["raven"]), linked("raven", &["quill"]), linked("quill", &[])];
        let meta = Meta { depth: 1, chain: vec!["atlas".into(), "raven".into()], origin: Some("atlas".into()) };
        let mut core = handoff_core(&team, "raven", "Ask QUILL too.\nHANDOFF quill: find my trip notes", meta);
        let ended = core.hear(5, Heard::Done(String::new()), &team).unwrap();
        assert_eq!(ended.delivery.as_ref().map(|delivery| delivery.meta.depth), Some(2));
        assert_eq!(ended.delivery.as_ref().map(|delivery| delivery.meta.chain.clone()), Some(vec!["atlas".to_owned(), "raven".to_owned(), "quill".to_owned()]));
        assert!(ended.changed.contains(&"quill".to_owned()) && ended.changed.contains(&"atlas".to_owned()));
        assert!(matches!(&core.chat(&team[0]).entries()[0].what, What::From { text, reply: true, .. } if text == "Ask QUILL too."));
    }
}
