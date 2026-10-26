# `autark-agentd`: the team's backend

Kith's agents (ATLAS, PIPER, the profession lenses and the ones you make) need what a team needs: a place where each
agent's job, conversation and memory live, a way for agents to hand work to each other, work that happens on a
schedule, and a record of what happened. `autark-agentd` is that backend, in the manner of agent-kitchen's `kitchend`:
one daemon owns the state and the rules, and every window or tool is a client of its API.

## What goes where

```
autark-kith (roster) ──┐
launcher / bar / notifyd ─┼─ D-Bus ─▶ autark-agentd  org.autark.Agents1
autark-agentd (tests) ──┘               │
                                        │ owns: agents, chats, memory, mailbox, schedule, events
                                        │
                              D-Bus ────▼──── autark-ai  org.autark.AI1
                                          the model and the senses, as before:
                                          Ollama (and Jev when switched on), mail, calendar,
                                          tasks, notes, web, all read-only
```

- **`autark-agentd`** decides who does what: which agent answers, what it may look at, what it remembers, who it may
  hand work to, and when it runs. It never talks to a model or reads your mail itself.
- **`autark-ai`** stays Kith's own daemon (the launcher's panel, routing, actions, tasks) and becomes the team's
  gateway to the model and the senses, through two calls: `Complete` (an answer for a given system prompt, streamed)
  and `Look` (what an agent may see about a question, from the subjects it is allowed). Everything private is read
  there, under the rules it already has.
- A client talks to `autark-agentd` only. The roster never calls `autark-ai` for an agent.

D-Bus is Autark's one way for its tools to talk, so the backend is `org.autark.Agents1` on the session bus rather than
a new kind of socket. The API is typed and listed below, and the daemon needs nothing from a client but the call.

## State

Files, like the rest of Autark, so you can read and back them up. All under your own account, mode 0600.

| What | Where |
|---|---|
| An agent: name, colour, job, soul, `can`, `skills`, `links` | `~/.config/autark-agentd/agents/<id>/agent.toml` |
| Its conversation (a week) | `~/.local/state/autark-agentd/agents/<id>/chat.json` |
| Its memory | `~/.config/autark-agentd/agents/<id>/memory.toml` |
| The event log (14 days) | `~/.local/state/autark-agentd/events.jsonl` |
| The mailbox (what is waiting to be delivered) | `~/.local/state/autark-agentd/mailbox.json` |

An id is a short slug, so it can't name a path outside the agents' folders.

## Memory

Each agent has a memory of its own, `memory.toml`, a `[[fact]]` for each, and it is never read by another agent. It is
small on purpose, as Hermes's and Kitchen's are: at most 12 facts and 2,000 characters in all. A fact that would go
past either limit is refused with the reason, not dropped to make room; you (or the agent's own proposal, which you
accept) forget one first. Facts are put in the agent's instructions each time it answers, so a fact forgotten is gone
at once. For now you add and forget facts yourself, from the roster's **⋯ › Memory…**; an agent told to remember
something says so and points you there, since a small model can't be trusted to claim a save it didn't make. Facts an
agent proposes, kept only when you accept them as with Kith's own memory, come later.

## Mailbox: agents handing work to each other

The rules are Kitchen's, because they are the ones that keep a team from tying itself in knots:

- **Identity comes from the daemon.** A message says who sent it because `autark-agentd` wrote it on that agent's
  behalf, never because a model claimed it. A message from you says so.
- **Only linked agents.** An agent's `links` names the agents it may hand work to. No link, no delivery, and the
  refusal is shown in the sender's chat so you can see why.
- **A depth limit.** A handoff carries how many handoffs led to it; at three, the next is refused, so two agents can't
  keep passing a question back and forth.
- **Visible.** Every handoff and every reply is an entry in both agents' conversations, marked with who it is from. You
  can follow a chain by reading the chats, and each is an event in the log.
- **Read-only.** A message can ask an agent to look something up and answer. It can't make an agent send, change or run
  anything: nothing an agent can do goes beyond what its `can` list lets it read, and anything that would change
  something comes to you first, as Kitchen's front of house does.
- **Code routes, not the model.** An agent hands work on by ending its reply with a line `HANDOFF <agent>: <brief>`.
  The daemon reads that line itself, checks the link and the depth, and delivers. A model that gets the line wrong
  hands nothing on.

## Schedule

An agent can have jobs: a prompt and a time (every morning at 8; weekdays at 17:30). The daemon runs them through the
same path as a message from you, in that agent's conversation, so the result is waiting when you open the roster, and
a missed run is made up on waking. Each agent may also have a **heartbeat**, as OpenClaw's: it wakes every so often,
code runs the cheap checks first (is there new mail? has the calendar changed?), and only when something changed does
the model get called. This is what makes a 4B model usable for standing work.

## API: `org.autark.Agents1` at `/org/autark/Agents1`

| Call | Does |
|---|---|
| `Agents()` | The team as JSON: each agent, its last words and time, whether it is writing |
| `CreateAgent(name, job, soul, can, skills, links)` / `DeleteAgent(id)` / `SetAgent(id, json)` | Make, remove, change |
| `History(id)` / `Say(id, text)` / `Clear(id)` / `Stop(answer)` | The conversation |
| `Memory(id)` / `Remember(id, text)` / `Forget(id, n)` | Its memory |
| `Events(since)` | What happened, as JSON lines |
| signals `AgentChanged(id)`, `AgentsChanged()`, `AnswerPiece(answer, offset, text)` | What clients follow |

## Privacy

Nothing leaves the laptop unless you have switched a cloud engine on in `ai.toml`, as before. The daemon keeps what you
said under your account only. A skill or an agent's job can't widen what it may look at: `can` is checked by code
before anything is read. The event log keeps who, what kind and when, and the first words of a message, never an
agent's whole answer.

## Order of work

1. **The backend and its state** (this PR): the daemon, agents, conversations, memory, the event log; `autark-ai`
   gains `Complete` and `Look`; the roster moves to `org.autark.Agents1`. `links` is stored and validated (it names
   agents on the team, never the agent itself) but does nothing until step 2, so the roster has no field for it yet.
   Kith stays `autark-ai`'s: a message to it from the roster goes through its own `Chat` and `ChatAnswer`, and what it
   would do as an action is left to the launcher's panel, where it asks first.
2. **The mailbox:** `links`, `HANDOFF`, depth limit, both chats showing it.
3. **The schedule and heartbeat.**
4. **Clients:** the launcher and the bar show a waiting agent; a Kitchen agent that reads `kitchend`.
