//! The team: named agents, each with a job, a soul, a memory and a conversation of its own. Kith itself is always the
//! first of them, kept by `autark-ai`: its conversation is the one the launcher shows. The others are folders of
//! `~/.config/autark-agentd/agents/<id>/` (`agent.toml`, `memory.toml`), with their conversation in
//! `$XDG_STATE_HOME/autark-agentd/agents/<id>/chat.json`: what one says and remembers is never mixed with another's.

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
/// The most skills one agent works by, and how much of each is read.
const MAX_SKILLS: usize = 4;
const SKILL: usize = 10 * 1024;
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
    /// Skills it works by, as `SKILL.md` folders (see `skill_text`): their text is part of its instructions.
    pub skills: Vec<String>,
    /// The agents it may hand work to, by id.
    pub links: Vec<String>,
}

/// What an agent can be allowed to look at, as `senses` names them.
pub const ABILITIES: [&str; 6] = ["mail", "calendar", "tasks", "notes", "web", "laptop"];

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
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    links: Vec<String>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("autark-agentd/agents")
}

/// Where the daemon keeps what it writes as it runs: the event log, the mailbox and each agent's conversation.
pub fn state_base() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state")).join("autark-agentd")
}

fn state_root() -> PathBuf {
    state_base().join("agents")
}

/// Where an agent's memory is kept, `None` for an id that isn't a slug.
pub fn memory_path(id: &str) -> Option<PathBuf> {
    (id != KITH && valid(id)).then(|| config_root().join(id).join("memory.toml"))
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
        skills: Vec::new(),
        links: Vec::new(),
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
            Some(Agent { id, name: file.name, colour: file.colour % COLOURS, job: file.job, soul: file.soul, can: file.can, skills: file.skills, links: file.links })
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
pub fn create(name: &str, job: &str, soul: &str, can: &[String], skills: &[String], links: &[String]) -> Result<Agent, String> {
    create_in(&config_root(), name, job, soul, can, skills, links)
}

/// What `SetAgent` may change; a field left out stays as it is. The name stays: a conversation says who spoke.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    job: Option<String>,
    soul: Option<String>,
    can: Option<Vec<String>>,
    skills: Option<Vec<String>>,
    links: Option<Vec<String>>,
}

/// Changes an agent's job, soul, abilities, skills or links, and returns it. Kith's can't be changed.
pub fn update(id: &str, patch: &Patch) -> Result<Agent, String> {
    update_in(&config_root(), id, patch)
}

fn update_in(root: &Path, id: &str, patch: &Patch) -> Result<Agent, String> {
    if id == KITH {
        return Err("Kith's job and soul are kept in its own files".to_owned());
    }
    let existing = all_in(root);
    let mut agent = existing.iter().find(|agent| agent.id == id).cloned().ok_or_else(|| format!("there is no agent {id}"))?;
    if let Some(job) = &patch.job {
        agent.job = job.trim().to_owned();
    }
    if let Some(soul) = &patch.soul {
        agent.soul = soul.trim().to_owned();
    }
    if agent.job.chars().count() > JOB || agent.soul.len() > SOUL {
        return Err("that job or soul is too long".to_owned());
    }
    if let Some(can) = &patch.can {
        agent.can = abilities(can);
    }
    if let Some(skills) = &patch.skills {
        agent.skills = slugs(skills);
    }
    if let Some(links) = &patch.links {
        agent.links = linked(links, &existing, id)?;
    }
    write(root, &agent)?;
    Ok(agent)
}

/// The abilities asked for that are real ones, in their usual order.
fn abilities(asked: &[String]) -> Vec<String> {
    ABILITIES.iter().filter(|ability| asked.iter().any(|asked| asked == *ability)).map(|ability| (*ability).to_owned()).collect()
}

fn slugs(skills: &[String]) -> Vec<String> {
    skills.iter().map(|skill| skill.trim().to_owned()).filter(|skill| valid(skill)).take(MAX_SKILLS).collect()
}

/// The agents to link to: ones on the team that are not `own`, each once. Kith isn't one: it takes requests in the
/// launcher's panel, where it asks first.
fn linked(links: &[String], existing: &[Agent], own: &str) -> Result<Vec<String>, String> {
    let mut found: Vec<String> = Vec::new();
    for link in links {
        let link = link.trim();
        if link == own {
            return Err("an agent can't be linked to itself".to_owned());
        }
        if link == KITH {
            return Err("work can't be handed to Kith: ask it in the launcher's panel".to_owned());
        }
        if !existing.iter().any(|agent| agent.id == link) {
            return Err(format!("there is no agent {link} to link to"));
        }
        if !found.iter().any(|seen| seen == link) {
            found.push(link.to_owned());
        }
    }
    Ok(found)
}

fn write(root: &Path, agent: &Agent) -> Result<(), String> {
    let file =
        File { name: agent.name.clone(), colour: agent.colour, job: agent.job.clone(), soul: agent.soul.clone(), can: agent.can.clone(), skills: agent.skills.clone(), links: agent.links.clone() };
    let dir = root.join(&agent.id);
    std::fs::create_dir_all(&dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    let text = toml::to_string(&file).map_err(|err| err.to_string())?;
    std::fs::write(dir.join("agent.toml"), text).map_err(|err| format!("writing {}: {err}", dir.display()))
}

fn create_in(root: &Path, name: &str, job: &str, soul: &str, can: &[String], skills: &[String], links: &[String]) -> Result<Agent, String> {
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
    let can = abilities(can);
    let skills = slugs(skills);
    let id = slug(name, &existing);
    let links = linked(links, &existing, &id)?;
    // Each new agent takes the colour least used, so a team looks varied.
    let colour = (0..COLOURS).min_by_key(|colour| existing.iter().filter(|agent| agent.colour == *colour).count()).unwrap_or(0);
    let agent = Agent { id, name: name.to_owned(), colour, job: job.trim().to_owned(), soul: soul.trim().to_owned(), can, skills, links };
    write(root, &agent)?;
    Ok(agent)
}

/// The team a new install starts with, once: a folder marks that it was made, so an agent deleted later stays gone.
/// Each looks at only what its job needs.
const STARTERS: [(&str, &str, &str, &[&str]); 4] = [
    ("ATLAS", "Plan the user's day: what is on the calendar and the task board, and what to do first. Be brief and put the first thing first.", "Calm and decisive.", &["calendar", "tasks", "notes"]),
    ("PIPER", "Keep the user's inbox at zero: say who has written and what needs a reply, most urgent first. You only read; you never send.", "Brisk and dry.", &["mail"]),
    ("RAVEN", "Look things up on the web for the user and say which site each fact comes from.", "Curious and exact.", &["web"]),
    ("QUILL", "Find what the user has written or said before in their notes and conversations, and bring it back word for word.", "Quiet and precise.", &["notes"]),
];

/// The profession lenses: an agent for each skill that is on this machine, made once. They look at nothing of
/// the user's: they work from what the user tells them.
const LENSES: [(&str, &str, &str); 6] = [
    ("LEDGER", "accountant", "Work the user's books or financial-statements question the way an accountant does, by your skill."),
    ("LEVY", "tax-advisor", "Work the user's tax question the way a tax advisor does, by your skill."),
    ("GAVEL", "lawyer", "Think the user's legal question through the way a lawyer opens a matter, by your skill."),
    ("PULSE", "doctor", "Work the user's health question the way a physician does, by your skill. You never diagnose."),
    ("HAVEN", "psychiatrist", "Work the user's mental-health question the way a psychiatrist opens an evaluation, by your skill."),
    ("FORGE", "engineer", "Work the user's design or failure problem in a professional engineer's order, by your skill."),
];

/// Makes the lens agents whose skills are installed, once.
pub fn seed_lenses() {
    seed_lenses_in(&config_root(), &skill_dirs());
}

fn seed_lenses_in(root: &Path, dirs: &[PathBuf]) {
    let marker = root.join(".seeded-lenses");
    if marker.exists() {
        return;
    }
    for (name, skill, job) in LENSES {
        if skill_in(dirs, skill).is_none() {
            continue;
        }
        if let Err(err) = create_in(root, name, job, "", &[], &[skill.to_owned()], &[]) {
            eprintln!("agentd: making the agent {name}: {err}");
        }
    }
    if std::fs::create_dir_all(root).and_then(|()| std::fs::write(marker, "")).is_err() {
        eprintln!("agentd: marking the lens agents as made in {}", root.display());
    }
}

/// Who the starter team may hand work to: the planner to the ones that read mail, the web and notes, and the accountant and
/// the tax advisor to each other. Given once, to an agent that has no links yet, so what you set yourself stays.
const STARTER_LINKS: [(&str, &[&str]); 3] = [("atlas", &["piper", "raven", "quill"]), ("ledger", &["levy"]), ("levy", &["ledger"])];

pub fn seed_links() {
    seed_links_in(&config_root());
}

fn seed_links_in(root: &Path) {
    let marker = root.join(".seeded-links");
    if marker.exists() {
        return;
    }
    let team = all_in(root);
    for (id, links) in STARTER_LINKS {
        let Some(agent) = team.iter().find(|agent| agent.id == id).filter(|agent| agent.links.is_empty()) else { continue };
        let links: Vec<String> = links.iter().filter(|link| team.iter().any(|other| other.id == **link)).map(|link| (*link).to_owned()).collect();
        if let Err(err) = update_in(root, &agent.id, &Patch { links: Some(links), ..Patch::default() }) {
            eprintln!("agentd: linking {}: {err}", agent.name);
        }
    }
    if std::fs::create_dir_all(root).and_then(|()| std::fs::write(marker, "")).is_err() {
        eprintln!("agentd: marking the starter links as made in {}", root.display());
    }
}

/// Where skills are looked for: a folder of your own for the agents, then Claude Code's.
fn skill_dirs() -> Vec<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    vec![config.join("autark-agentd/skills-md"), home().join(".claude/skills")]
}

/// A skill's text: its `SKILL.md` without the front matter, at most 10 KB. `None` when it isn't there.
pub fn skill_text(name: &str) -> Option<String> {
    skill_in(&skill_dirs(), name)
}

fn skill_in(dirs: &[PathBuf], name: &str) -> Option<String> {
    if !valid(name) {
        return None;
    }
    let text = dirs.iter().find_map(|dir| std::fs::read_to_string(dir.join(name).join("SKILL.md")).ok())?;
    let body = match text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")) {
        Some((_, body)) => body,
        None => &text,
    };
    let body = without_sources(body);
    let body = body.trim();
    let mut end = body.len().min(SKILL);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    Some(body[..end].to_owned())
}

/// The skill's own `[sourced C7, C8]` notes for its authors, which a small model would copy into its answers.
fn without_sources(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(" [sourced ") {
        out.push_str(&rest[..start]);
        match rest[start..].find(']') {
            Some(end) => rest = &rest[start + end + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

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
        if let Err(err) = create_in(root, name, job, soul, &can, &[], &[]) {
            eprintln!("agentd: making the agent {name}: {err}");
        }
    }
    if std::fs::create_dir_all(root).and_then(|()| std::fs::write(marker, "")).is_err() {
        eprintln!("agentd: marking the starter team as made in {}", root.display());
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
        let dir = std::env::temp_dir().join(format!("agentd-agents-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn kith_is_first_and_the_rest_are_by_name() {
        let root = folder("order");
        create_in(&root, "Sales Outbound", "Draft outreach.", "", &[], &[], &[]).unwrap();
        create_in(&root, "Inbox Manager", "Keep the inbox at zero.", "Brisk.", &[], &[], &[]).unwrap();
        let names: Vec<String> = all_in(&root).into_iter().map(|agent| agent.name).collect();
        assert_eq!(names, ["Kith", "Inbox Manager", "Sales Outbound"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_agent_is_kept_with_its_job_and_soul() {
        let root = folder("kept");
        create_in(&root, "Expense Manager", "File receipts.", "Short and exact.", &["mail".to_owned(), "bogus".to_owned()], &[], &[]).unwrap();
        let agent = all_in(&root).into_iter().find(|agent| agent.id == "expense-manager").unwrap();
        assert_eq!((agent.job.as_str(), agent.soul.as_str()), ("File receipts.", "Short and exact."));
        // Only a real ability is kept, and it is all the agent may look at.
        assert_eq!(agent.can, ["mail"]);
        assert_eq!(kith().can.len(), ABILITIES.len());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_name_is_not_used_twice_and_never_takes_kiths_id() {
        let root = folder("names");
        create_in(&root, "Scout", "", "", &[], &[], &[]).unwrap();
        assert!(create_in(&root, "scout", "", "", &[], &[], &[]).is_err());
        assert_eq!(create_in(&root, "Kith!", "", "", &[], &[], &[]).unwrap().id, "kith-2");
        assert_eq!(create_in(&root, "Scout?", "", "", &[], &[], &[]).unwrap().id, "scout-2");
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
        let agent = create_in(&root, "Talent Scout", "", "", &[], &[], &[]).unwrap();
        assert!(delete_in(&root, &root, KITH).is_err());
        delete_in(&root, &root, &agent.id).unwrap();
        assert_eq!(all_in(&root).len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn new_agents_take_different_colours() {
        let root = folder("colours");
        let colours: Vec<u8> = ["A", "B", "C"].iter().map(|name| create_in(&root, name, "", "", &[], &[], &[]).unwrap().colour).collect();
        assert_eq!(colours.len(), 3);
        assert!(colours[0] != colours[1] && colours[1] != colours[2] && colours[0] != colours[2]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_starter_team_is_made_once_and_stays_gone_when_deleted() {
        let root = folder("seed");
        seed_in(&root);
        let names: Vec<String> = all_in(&root).into_iter().map(|agent| agent.name).collect();
        assert_eq!(names, ["Kith", "ATLAS", "PIPER", "QUILL", "RAVEN"]);
        delete_in(&root, &root, "raven").unwrap();
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

    #[test]
    fn a_skill_is_read_without_its_front_matter_and_only_by_a_slug() {
        let dir = folder("skill");
        std::fs::create_dir_all(dir.join("lawyer")).unwrap();
        std::fs::write(dir.join("lawyer/SKILL.md"), "---\nname: lawyer\ndescription: x\n---\n\n# Lawyer\nAsk first.\n").unwrap();
        let dirs = [dir.clone()];
        assert_eq!(skill_in(&dirs, "lawyer").as_deref(), Some("# Lawyer\nAsk first."));
        assert_eq!(skill_in(&dirs, "missing"), None);
        assert_eq!(skill_in(&dirs, "../lawyer"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_long_skill_is_cut_at_a_character() {
        let dir = folder("long");
        std::fs::create_dir_all(dir.join("big")).unwrap();
        std::fs::write(dir.join("big/SKILL.md"), "é".repeat(SKILL)).unwrap();
        assert!(skill_in(std::slice::from_ref(&dir), "big").unwrap().len() <= SKILL);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lens_agents_are_made_only_for_skills_that_are_installed_and_only_once() {
        let skills = folder("lenses-skills");
        let root = folder("lenses");
        for name in ["accountant", "lawyer"] {
            std::fs::create_dir_all(skills.join(name)).unwrap();
            std::fs::write(skills.join(name).join("SKILL.md"), "# x").unwrap();
        }
        seed_lenses_in(&root, std::slice::from_ref(&skills));
        let made: Vec<(String, Vec<String>)> = all_in(&root).into_iter().skip(1).map(|agent| (agent.name, agent.skills)).collect();
        assert_eq!(made, [("GAVEL".to_owned(), vec!["lawyer".to_owned()]), ("LEDGER".to_owned(), vec!["accountant".to_owned()])]);
        delete_in(&root, &root, "gavel").unwrap();
        seed_lenses_in(&root, std::slice::from_ref(&skills));
        assert_eq!(all_in(&root).len(), 2);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(skills);
    }

    #[test]
    fn every_lens_names_a_skill_slug() {
        for (_, skill, _) in LENSES {
            assert!(valid(skill));
        }
    }

    #[test]
    fn the_authors_source_notes_are_left_out_of_a_skill() {
        assert_eq!(without_sources("Ask the country. [sourced C7, C8] Then the entity. [sourced C9]\nDone."), "Ask the country. Then the entity.\nDone.");
        assert_eq!(without_sources("no notes"), "no notes");
        assert_eq!(without_sources("open [sourced C1"), "open");
    }

    #[test]
    fn links_name_agents_on_the_team_and_never_the_agent_itself() {
        let root = folder("links");
        create_in(&root, "Piper", "", "", &[], &[], &[]).unwrap();
        let raven = create_in(&root, "Raven", "", "", &[], &[], &[]).unwrap();
        let atlas = create_in(&root, "Atlas", "", "", &[], &[], &["piper".to_owned(), raven.id.clone(), "piper".to_owned()]).unwrap();
        assert_eq!(atlas.links, ["piper", "raven"]);
        assert!(create_in(&root, "Quill", "", "", &[], &[], &["kith".to_owned()]).unwrap_err().contains("Kith"));
        assert!(create_in(&root, "Scout", "", "", &[], &[], &["nobody".to_owned()]).is_err());
        assert!(update_in(&root, "atlas", &Patch { links: Some(vec!["atlas".to_owned()]), ..Patch::default() }).is_err());
        assert_eq!(all_in(&root).iter().find(|agent| agent.id == "atlas").unwrap().links, ["piper", "raven"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_agent_is_changed_a_field_at_a_time_and_kith_is_not() {
        let root = folder("update");
        create_in(&root, "Piper", "Read mail.", "Brisk.", &["mail".to_owned()], &[], &[]).unwrap();
        let piper = update_in(
            &root,
            "piper",
            &Patch { job: Some(" Read mail, then calendar. ".to_owned()), can: Some(vec!["calendar".to_owned(), "mail".to_owned(), "bogus".to_owned()]), ..Patch::default() },
        )
        .unwrap();
        assert_eq!((piper.job.as_str(), piper.soul.as_str()), ("Read mail, then calendar.", "Brisk."));
        assert_eq!(piper.can, ["mail", "calendar"]);
        assert_eq!(all_in(&root).iter().find(|agent| agent.id == "piper").unwrap(), &piper);
        assert!(update_in(&root, KITH, &Patch::default()).is_err());
        assert!(update_in(&root, "nobody", &Patch::default()).is_err());
        assert!(update_in(&root, "piper", &Patch { soul: Some("x".repeat(SOUL + 1)), ..Patch::default() }).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_starter_links_are_given_once_to_agents_without_any() {
        let root = folder("starter-links");
        seed_in(&root);
        create_in(&root, "Ledger", "", "", &[], &[], &[]).unwrap();
        create_in(&root, "Levy", "", "", &[], &[], &["ledger".to_owned()]).unwrap();
        seed_links_in(&root);
        let links = |id: &str| all_in(&root).into_iter().find(|agent| agent.id == id).unwrap().links;
        assert_eq!(links("atlas"), ["piper", "raven", "quill"]);
        assert_eq!(links("ledger"), ["levy"]);
        // One that had links of its own keeps them, and nothing is given a second time.
        assert_eq!(links("levy"), ["ledger"]);
        update_in(&root, "atlas", &Patch { links: Some(vec![]), ..Patch::default() }).unwrap();
        seed_links_in(&root);
        assert!(links("atlas").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
