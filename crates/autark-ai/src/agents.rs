//! Kith's team: named agents, each with a job, a soul and a conversation of its own. Kith itself is always the
//! first of them, with the soul in `soul.md` and the conversation the launcher shows. The others are folders of
//! `~/.config/autark-ai/agents/<id>/agent.toml`, with their conversation in
//! `$XDG_STATE_HOME/autark-ai/agents/<id>/chat.json`: what one says and remembers is never mixed with another's.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The id of Kith itself.
pub const KITH: &str = "kith";

/// The most agents besides Kith, so a loop of requests can't fill the folder.
const MAX: usize = 24;
/// How long a name, a job and a soul may be.
const NAME: usize = 40;
const JOB: usize = 600;
const SOUL: usize = 4 * 1024;
/// How many avatar colours the roster has.
pub const COLOURS: u8 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub id: String,
    pub name: String,
    /// Which of the roster's avatar colours, 0 to `COLOURS`.
    pub colour: u8,
    /// What the agent is for, said as an instruction to it.
    pub job: String,
    /// How it talks, as Kith's `soul.md` does.
    pub soul: String,
    /// What it may look at, of `ABILITIES`. Everything it looks at is read-only: it can't send, change or run
    /// anything. Kith itself may look at all of them.
    pub can: Vec<String>,
}

/// What an agent can be allowed to look at, as `senses` names them.
pub const ABILITIES: [&str; 6] = ["mail", "calendar", "tasks", "notes", "web", "laptop"];

impl Agent {
    pub fn allows(&self, ability: &str) -> bool {
        self.id == KITH || self.can.iter().any(|can| can == ability)
    }
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    name: String,
    #[serde(default)]
    colour: u8,
    #[serde(default)]
    job: String,
    #[serde(default)]
    soul: String,
    #[serde(default)]
    can: Vec<String>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("autark-ai/agents")
}

fn state_root() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state")).join("autark-ai/agents")
}

/// Where an agent's conversation is kept. Kith's is the one the launcher shows.
pub fn chat_path(id: &str) -> Option<PathBuf> {
    (id != KITH && valid(id)).then(|| state_root().join(id).join("chat.json"))
}

/// An id is a short slug, so it can't name a path outside the agents' folders.
fn valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') && !id.starts_with('-')
}

fn kith() -> Agent {
    Agent {
        id: KITH.to_owned(),
        name: "Kith".to_owned(),
        colour: 0,
        job: "Your assistant: it answers, does small things on the laptop and remembers what you tell it.".to_owned(),
        soul: String::new(),
        can: ABILITIES.map(str::to_owned).to_vec(),
    }
}

/// Kith, then the other agents by name.
pub fn all() -> Vec<Agent> {
    all_in(&config_root())
}

fn all_in(root: &Path) -> Vec<Agent> {
    let mut found: Vec<Agent> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok().filter(|id| valid(id) && id != KITH)?;
            let file: File = toml::from_str(&std::fs::read_to_string(entry.path().join("agent.toml")).ok()?).ok()?;
            Some(Agent { id, name: file.name, colour: file.colour % COLOURS, job: file.job, soul: file.soul, can: file.can })
        })
        .collect();
    found.sort_by_key(|agent| agent.name.to_lowercase());
    found.truncate(MAX);
    found.insert(0, kith());
    found
}

pub fn get(id: &str) -> Option<Agent> {
    all().into_iter().find(|agent| agent.id == id)
}

/// Makes an agent, and returns it. The id comes from the name.
pub fn create(name: &str, job: &str, soul: &str, can: &[String]) -> Result<Agent, String> {
    create_in(&config_root(), name, job, soul, can)
}

fn create_in(root: &Path, name: &str, job: &str, soul: &str, can: &[String]) -> Result<Agent, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME {
        return Err(format!("an agent's name is 1 to {NAME} characters"));
    }
    if job.chars().count() > JOB || soul.len() > SOUL {
        return Err("that job or soul is too long".to_owned());
    }
    let existing = all_in(root);
    if existing.len() > MAX {
        return Err(format!("there are {MAX} agents already: delete one first"));
    }
    if existing.iter().any(|agent| agent.name.eq_ignore_ascii_case(name)) {
        return Err(format!("there is an agent called {name} already"));
    }
    let can: Vec<String> = ABILITIES.iter().filter(|ability| can.iter().any(|asked| asked == *ability)).map(|ability| (*ability).to_owned()).collect();
    let id = slug(name, &existing);
    // Each new agent takes the colour least used, so a team looks varied.
    let colour = (0..COLOURS).min_by_key(|colour| existing.iter().filter(|agent| agent.colour == *colour).count()).unwrap_or(0);
    let agent = Agent { id: id.clone(), name: name.to_owned(), colour, job: job.trim().to_owned(), soul: soul.trim().to_owned(), can };
    let file = File { name: agent.name.clone(), colour, job: agent.job.clone(), soul: agent.soul.clone(), can: agent.can.clone() };
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    let text = toml::to_string(&file).map_err(|err| err.to_string())?;
    std::fs::write(dir.join("agent.toml"), text).map_err(|err| format!("writing {}: {err}", dir.display()))?;
    Ok(agent)
}

/// The team a new install starts with, once: a folder marks that it was made, so an agent deleted later stays gone.
/// Each looks at only what its job needs.
const STARTERS: [(&str, &str, &str, &[&str]); 4] = [
    ("Chief", "Plan the user's day: what is on the calendar and the task board, and what to do first. Be brief and put the first thing first.", "Calm and decisive.", &["calendar", "tasks", "notes"]),
    ("Inbox", "Keep the user's inbox at zero: say who has written and what needs a reply, most urgent first. You only read; you never send.", "Brisk and dry.", &["mail"]),
    ("Scout", "Look things up on the web for the user and say which site each fact comes from.", "Curious and exact.", &["web"]),
    ("Scribe", "Find what the user has written or said before in their notes and conversations, and bring it back word for word.", "Quiet and precise.", &["notes"]),
];

/// Makes the starter team when this install hasn't had one.
pub fn seed() {
    seed_in(&config_root());
}

fn seed_in(root: &Path) {
    let marker = root.join(".seeded");
    if marker.exists() {
        return;
    }
    for (name, job, soul, can) in STARTERS {
        let can: Vec<String> = can.iter().map(|can| (*can).to_owned()).collect();
        if let Err(err) = create_in(root, name, job, soul, &can) {
            eprintln!("ai: making the agent {name}: {err}");
        }
    }
    if std::fs::create_dir_all(root).and_then(|()| std::fs::write(marker, "")).is_err() {
        eprintln!("ai: marking the starter team as made in {}", root.display());
    }
}

/// Removes an agent and its conversation. Kith can't be removed.
pub fn delete(id: &str) -> Result<(), String> {
    delete_in(&config_root(), &state_root(), id)
}

fn delete_in(root: &Path, state: &Path, id: &str) -> Result<(), String> {
    if id == KITH {
        return Err("Kith can't be deleted".to_owned());
    }
    if !valid(id) || !root.join(id).join("agent.toml").is_file() {
        return Err(format!("there is no agent {id}"));
    }
    std::fs::remove_dir_all(root.join(id)).map_err(|err| format!("removing {id}: {err}"))?;
    // Its conversation goes too.
    let _ = std::fs::remove_dir_all(state.join(id));
    Ok(())
}

/// A name as an id: lowercase letters, digits and dashes, and not one that is taken.
fn slug(name: &str, existing: &[Agent]) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let mut slug: String = slug.trim_end_matches('-').chars().take(24).collect();
    if slug.is_empty() {
        slug = "agent".to_owned();
    }
    let taken = |id: &str| id == KITH || existing.iter().any(|agent| agent.id == id);
    let base = slug.clone();
    let mut n = 2;
    while taken(&slug) {
        slug = format!("{base}-{n}");
        n += 1;
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("autark-ai-agents-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn kith_is_first_and_the_rest_are_by_name() {
        let root = folder("order");
        create_in(&root, "Sales Outbound", "Draft outreach.", "", &[]).unwrap();
        create_in(&root, "Inbox Manager", "Keep the inbox at zero.", "Brisk.", &[]).unwrap();
        let names: Vec<String> = all_in(&root).into_iter().map(|agent| agent.name).collect();
        assert_eq!(names, ["Kith", "Inbox Manager", "Sales Outbound"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_agent_is_kept_with_its_job_and_soul() {
        let root = folder("kept");
        create_in(&root, "Expense Manager", "File receipts.", "Short and exact.", &["mail".to_owned(), "bogus".to_owned()]).unwrap();
        let agent = all_in(&root).into_iter().find(|agent| agent.id == "expense-manager").unwrap();
        assert_eq!((agent.job.as_str(), agent.soul.as_str()), ("File receipts.", "Short and exact."));
        // Only a real ability is kept, and it is all the agent may look at.
        assert_eq!(agent.can, ["mail"]);
        assert!(agent.allows("mail") && !agent.allows("calendar") && kith().allows("calendar"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_name_is_not_used_twice_and_never_takes_kiths_id() {
        let root = folder("names");
        create_in(&root, "Scout", "", "", &[]).unwrap();
        assert!(create_in(&root, "scout", "", "", &[]).is_err());
        assert_eq!(create_in(&root, "Kith!", "", "", &[]).unwrap().id, "kith-2");
        assert_eq!(create_in(&root, "Scout?", "", "", &[]).unwrap().id, "scout-2");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_id_cannot_name_a_path_outside_the_agents_folder() {
        assert!(!valid("../x") && !valid("a/b") && !valid("") && !valid("-a") && !valid("A"));
        assert!(chat_path("../x").is_none());
        assert!(chat_path(KITH).is_none());
        let root = folder("escape");
        assert!(delete_in(&root, &root, "../x").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn kith_cannot_be_deleted_and_others_can() {
        let root = folder("delete");
        let agent = create_in(&root, "Talent Scout", "", "", &[]).unwrap();
        assert!(delete_in(&root, &root, KITH).is_err());
        delete_in(&root, &root, &agent.id).unwrap();
        assert_eq!(all_in(&root).len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn new_agents_take_different_colours() {
        let root = folder("colours");
        let colours: Vec<u8> = ["A", "B", "C"].iter().map(|name| create_in(&root, name, "", "", &[]).unwrap().colour).collect();
        assert_eq!(colours.len(), 3);
        assert!(colours[0] != colours[1] && colours[1] != colours[2] && colours[0] != colours[2]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_starter_team_is_made_once_and_stays_gone_when_deleted() {
        let root = folder("seed");
        seed_in(&root);
        let names: Vec<String> = all_in(&root).into_iter().map(|agent| agent.name).collect();
        assert_eq!(names, ["Kith", "Chief", "Inbox", "Scout", "Scribe"]);
        delete_in(&root, &root, "scout").unwrap();
        seed_in(&root);
        assert_eq!(all_in(&root).len(), 4);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_starters_abilities_are_real_ones() {
        for (_, _, _, can) in STARTERS {
            assert!(can.iter().all(|can| ABILITIES.contains(can)));
        }
    }
}
