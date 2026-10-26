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
at once. You add and forget facts from the roster's **⋯ › Memory…**, and an agent proposes one when a sentence starts by asking
it to remember ("remember that my wife's birthday is May 3", "please remember: …", "don't forget …"). Code reads the
sentence, not the model, so a small model can't claim a save it didn't make: the agent answers that it can keep it if
you say so, and a card under it offers **Save** or **Don't save**. Nothing is kept unless you save it, as with Kith's
own memory. A question ("do you remember my name?") isn't a fact, and a fact that won't fit says why and waits.

## Adding to a note

"Add milk, eggs and bread to my grocery list" (or "put … on the shopping list", "add … to my work notes") is read by
code, as "remember that" is. An agent that may look at notes (`can` has `notes`) answers that it can add it if you say
so, and a card names the note and the lines with **Add** and **Don't add**. An agent that may not says so and does
nothing. Only when you tap Add is anything written, and what is written is only lines (`- [ ] milk`) added at the end
of a Markdown note in your Notes vault, the one Notes opens: a note of that name anywhere in the vault (three folders
down, dot-folders skipped, case ignored), else a new one in its top folder. It never changes or removes what is in a
note, never leaves the vault, refuses a note named for a secret (password, token, …) or over 256 KB, and takes at most
10 lines of 200 characters from one sentence. A sentence that isn't about a list or a note ("add a meeting to my
calendar") is not taken for one. This is the first thing an agent does beyond reading, and the tap is its gate, as
Kitchen's front of house gates a change.

## Mailbox: agents handing work to each other

The rules are Kitchen's, because they are the ones that keep a team from tying itself in knots:

- **Identity comes from the daemon.** A message says who sent it because `autark-agentd` wrote it on that agent's
  behalf, never because a model claimed it. A message from you says so.
- **Only linked agents.** An agent's `links` names the agents it may hand work to ("Hands work to…" in the roster's
  **⋯**, or when making an agent). No link, no handoff, and the refusal is shown in the sender's chat so you can see why.
  Kith can't be linked: it takes requests in the launcher's panel, where it asks first. A new install links ATLAS to
  PIPER, RAVEN and QUILL, and LEDGER and LEVY to each other, once.
- **A depth limit, and no loops.** A handoff carries how many handoffs led to it; an answer three deep can't hand on,
  and a chain never returns to an agent already in it, so two agents can't keep passing a question back and forth. An
  agent that is busy answering isn't handed work: the refusal says so.
- **Visible.** A handoff is a line in the sender's conversation ("Asked RAVEN: …") and a message "ATLAS asked" in the
  receiver's, which answers like any other; its answer goes back to the sender's conversation as "RAVEN replied". Each is
  an event in the log. The sender doesn't answer again: you read the reply where it lands, and may follow up.
- **Read-only.** What is handed over is a request to look something up and answer. The receiver looks at no more than
  its own `can` list, and nothing it can do goes beyond reading, apart from the additions to a note you tap Add on.
- **Code routes, not the model.** An agent with links is told who they are and to end its reply with a line
  `HANDOFF <id>: <what to ask>`. The daemon reads that line itself (only the last line, only when well formed), takes it
  out of what is shown, checks the link, the depth, the chain and whether the other agent is busy, and delivers. A model
  that gets the line wrong hands nothing on, and one that claims it handed something without the line is wrong in a way
  you can see: there is no "Asked …" line.

## Schedule

An agent can have jobs (up to 5): a time and what to do, which the daemon does as if you had said it, in that agent's
conversation, so the result is waiting when you open the roster. A job is shown there as your request with "Scheduled ·
weekdays at 8:00" over it.

- **Code reads the time, not the model.** `every day at 8`, `weekdays at 17:30`, `weekends at 10am`, `every monday at 9`,
  `the 1st of every month at 9` (the 1st to the 28th, so every month has it), or `every morning` (8), `every evening` (18),
  `every night` (21). A time that can't be read says what can be said. A 4B model gets dates wrong, and a job on the wrong
  day is worse than none.
- **Set from the roster** (**⋯ › Schedule…**: when, what, an on/off switch, Run now, remove) **or by saying it:** "every
  weekday at 8, tell me who has written" (or the same with the time last) answers with a card, **Schedule** or **Don't
  schedule**, and only on Schedule is a job made. A sentence about yourself ("I go to the gym every day at 6") or a
  question isn't one.
- **A missed run is made up once.** Each job keeps the time it was last due and run (`jobs.toml`). Every half minute, and
  soon after the daemon starts, anything due after that runs once, however many times it came while the laptop slept. A job
  made now doesn't run for a time before it was made, and one switched back on starts from then.
- **An agent that is busy answering waits:** the run happens at the next look after it is free. A run that fails (the model
  is off) is not tried again until its next time.
- **It is read-only like any answer:** the agent looks only at its `can` list, and a job can hand work on, within the
  mailbox's limits.

## Telling you

An answer you didn't ask for just now is the one you'd miss: a scheduled run's, or an agent's reply to work it handed on.
When one lands, `autark-agentd` puts a card in the notification centre, quietly (the same `x-autark-quiet` card Kith's
other suggestions are: no banner, no chime, so Focus and a presentation are not broken into), titled with the agent
("QUILL · scheduled", or the asker's name for a reply) and holding the first 140 characters: "RAVEN replied: Friday is
free." A run that failed says so ("Couldn't run: Ollama isn't running"), so a job doesn't go quiet. The roster puts a dot and
bold words on that agent's row until you open the conversation with the window in front (or come back to it with that
conversation open); `MarkRead` is what tells the daemon. An answer to what you said yourself is never news, and neither
is the first half of a handoff, only the reply. Which agents are unread is kept while the daemon runs, not across a restart:
the conversation still holds the answer. Clicking a card doesn't open the roster yet.

Heartbeats, as OpenClaw's, are next: an agent wakes every so often, code runs the cheap checks first (is there new mail? has
the calendar changed?), and only when something changed does the model get called. That is what makes a 4B model usable for
standing work, and it needs those checks to exist in `autark-ai` first.

## API: `org.autark.Agents1` at `/org/autark/Agents1`

| Call | Does |
|---|---|
| `Agents()` | The team as JSON: each agent, its last words and time, whether it is writing |
| `CreateAgent(name, job, soul, can, skills, links)` / `DeleteAgent(id)` / `SetAgent(id, json)` | Make, remove, change (`links` too) |
| `History(id)` / `Say(id, text)` / `Clear(id)` / `Stop(answer)` | The conversation |
| `Memory(id)` / `Remember(id, text)` / `Forget(id, n)` | Its memory |
| `Jobs(id)` / `AddJob(id, when, prompt)` / `RemoveJob(id, n)` / `SetJobEnabled(id, n, on)` / `RunJob(id, n)` | Its schedule |
| `MarkRead(id)` | You've seen what an agent said while you weren't looking; the `unread` flag in `Agents()` goes |
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
2. **The mailbox** (stacked on the first): `links`, `HANDOFF`, depth limit, both chats showing it.
3. **The schedule** (stacked on the mailbox), then **heartbeats**.
4. **Clients:** a quiet card and an unread dot when an answer lands (the notification-centre side is done; the launcher
   and the bar showing a waiting agent are not), and a Kitchen agent that reads `kitchend`.
