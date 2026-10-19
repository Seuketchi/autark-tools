//! Kith's skills: a multi-step look that worked, kept so the next time it is asked for in the same words, Kith
//! does the same steps without working them out again. After a question that took two or more tools (see
//! `agent`), Kith proposes the steps as a skill; nothing is saved unless you say so, as with memory.
//!
//! A skill is only a list of read-only tools and their inputs plus the words that call for it, in
//! `~/.config/autark-ai/skills/<name>.toml`. It cannot hold an action or a command, so a saved skill can't
//! change the laptop. Delete the file to forget the skill.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::Note;
use crate::recall;

/// How many skills Kith keeps, so a loop of proposals can't fill the folder.
const MAX: usize = 50;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    /// The question it was made from; its distinctive words are what call for the skill.
    pub when: String,
    pub steps: Vec<Step>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Step {
    pub tool: String,
    #[serde(default)]
    pub input: String,
}

pub fn folder() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("autark-ai/skills")
}

/// Every saved skill, by file name. A file that doesn't read is skipped, with a line on stderr.
pub fn all() -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(folder()) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "toml")).collect();
    files.sort();
    files
        .iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            match toml::from_str::<Skill>(&text) {
                Ok(skill) if valid(&skill).is_ok() => Some(skill),
                Ok(skill) => {
                    eprintln!("ai: {}: {}", path.display(), valid(&skill).unwrap_err());
                    None
                }
                Err(err) => {
                    eprintln!("ai: {}: {err}", path.display());
                    None
                }
            }
        })
        .collect()
}

/// The tools a skill may name: only the loop's read-only ones, and the web only with something to search for.
pub fn valid(skill: &Skill) -> Result<(), String> {
    if skill.name.trim().is_empty() || skill.name.chars().count() > 60 || skill.name.contains('\n') {
        return Err("a skill needs a name of up to 60 characters".to_owned());
    }
    if skill.steps.len() < 2 || skill.steps.len() > crate::agent::MAX_STEPS {
        return Err(format!("a skill has 2 to {} steps", crate::agent::MAX_STEPS));
    }
    for step in &skill.steps {
        if crate::agent::tool_name(&step.tool).is_none() {
            return Err(format!("“{}” isn't a tool a skill can use", step.tool));
        }
        if matches!(step.tool.as_str(), "web" | "recall" | "read_file") && step.input.trim().is_empty() {
            return Err(format!("the {} step needs something to look for", step.tool));
        }
    }
    if recall::words(&skill.when).is_empty() {
        return Err("a skill needs words that call for it".to_owned());
    }
    Ok(())
}

fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    if slug.is_empty() { "skill".to_owned() } else { slug }
}

/// Saves a skill as its own file, numbered if the name is taken. Returns the file's path.
pub fn save(skill: &Skill) -> Result<PathBuf, String> {
    valid(skill)?;
    if all().len() >= MAX {
        return Err(format!("there are {MAX} skills already, the most Kith keeps"));
    }
    let dir = folder();
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let base = slug(&skill.name);
    let path = (1..).map(|n| dir.join(if n == 1 { format!("{base}.toml") } else { format!("{base}-{n}.toml") })).find(|path| !path.exists()).expect("an endless range");
    let text = toml::to_string(skill).map_err(|err| err.to_string())?;
    std::fs::write(&path, text).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

/// The skill a question calls for: it has the skill's distinctive words, three quarters of them and at least two,
/// so a skill runs when it's asked for in much the same words and not on a passing mention.
pub fn matching(question: &str, skills: &[Skill]) -> Option<Skill> {
    let asked = recall::words(question);
    let lower = question.to_lowercase();
    // Naming a skill calls for it, whatever else the sentence says.
    if let Some(named) = skills.iter().filter(|skill| skill.name.chars().count() >= 8 && lower.contains(&skill.name.to_lowercase())).max_by_key(|skill| skill.name.len()) {
        return Some(named.clone());
    }
    skills
        .iter()
        .filter_map(|skill| {
            let wanted = recall::words(&skill.when);
            let have = wanted.iter().filter(|word| asked.contains(word)).count();
            (have >= 2.max((wanted.len() * 3).div_ceil(4)) || (wanted.len() == 1 && have == 1)).then_some((have, skill))
        })
        .max_by_key(|(have, _)| *have)
        .map(|(_, skill)| skill.clone())
}

/// A skill to offer after a question's steps: two or more that all worked, for a question no skill covers yet.
pub fn propose(question: &str, notes: &[Note], saved: &[Skill]) -> Option<Skill> {
    if notes.len() < 2 || notes.iter().any(|note| note.result.starts_with("failed:")) || matching(question, saved).is_some() {
        return None;
    }
    let steps: Vec<Step> = notes.iter().map(|note| Step { tool: note.tool.clone(), input: note.input.clone() }).collect();
    if saved.iter().any(|skill| skill.steps == steps) {
        return None;
    }
    let skill = Skill { name: name_of(question), when: question.trim().chars().take(200).collect(), steps };
    valid(&skill).ok().map(|()| skill)
}

/// A skill's name from the question that made it: "How much disk is left and what's the latest kernel news?" is
/// "How much disk is left and what's the latest kernel news", cut at a word if long.
fn name_of(question: &str) -> String {
    let text = question.trim().trim_end_matches(['?', '.', '!']).trim();
    let mut name: String = text.chars().take(50).collect();
    if text.chars().count() > 50 {
        name = name.rsplit_once(' ').map_or(name.clone(), |(head, _)| head.to_owned());
    }
    let mut chars = name.chars();
    chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(tool: &str, input: &str, result: &str) -> Note {
        Note { tool: tool.to_owned(), input: input.to_owned(), result: result.to_owned() }
    }

    fn skill(when: &str, steps: &[(&str, &str)]) -> Skill {
        Skill { name: name_of(when), when: when.to_owned(), steps: steps.iter().map(|(tool, input)| Step { tool: (*tool).to_owned(), input: (*input).to_owned() }).collect() }
    }

    const Q: &str = "how much disk is left and what's the latest linux kernel news?";

    #[test]
    fn a_question_that_took_two_tools_is_offered_as_a_skill() {
        let notes = [note("system", "", "disk 80%"), note("web", "what's the latest linux kernel news", "kernel.org …")];
        let proposed = propose(Q, &notes, &[]).unwrap();
        assert_eq!(proposed.name, "How much disk is left and what's the latest linux", "cut at a word");
        assert_eq!(proposed.steps, vec![Step { tool: "system".into(), input: String::new() }, Step { tool: "web".into(), input: "what's the latest linux kernel news".into() }]);
        assert!(propose(Q, &notes[..1], &[]).is_none(), "one tool isn't a skill");
        assert!(propose(Q, &[note("system", "", "ok"), note("web", "x y z", "failed: no key")], &[]).is_none(), "a step that failed isn't worth keeping");
        assert!(propose(Q, &notes, &[proposed.clone()]).is_none(), "already saved");
        assert!(propose("disk and kernel news in other words please", &notes, &[proposed]).is_none(), "the same steps are already a skill");
    }

    #[test]
    fn a_skill_runs_when_asked_for_in_much_the_same_words() {
        let saved = [skill(Q, &[("system", ""), ("web", "latest linux kernel news")])];
        assert!(matching("how much disk is left, and the latest linux kernel news please", &saved).is_some());
        assert!(matching("what's the latest linux kernel news", &saved).is_none(), "a passing mention of part of it isn't enough");
        assert!(matching("why is the sky blue", &saved).is_none());
        assert!(matching("anything", &[]).is_none());
    }

    #[test]
    fn naming_a_skill_calls_for_it() {
        let saved = [skill(Q, &[("system", ""), ("web", "latest linux kernel news")])];
        let named = Skill { name: "Disk and kernel news".into(), ..saved[0].clone() };
        assert!(matching("run my disk and kernel news every morning", std::slice::from_ref(&named)).is_some());
        assert!(matching("run my disk news", &[named]).is_none());
    }

    #[test]
    fn a_skill_holds_only_the_loops_read_only_tools() {
        let good = skill(Q, &[("system", ""), ("recall", "kernel plan")]);
        assert!(valid(&good).is_ok());
        assert!(valid(&skill(Q, &[("system", ""), ("run_actions", "mute")])).is_err(), "no actions in a skill");
        assert!(valid(&skill(Q, &[("system", ""), ("web", "")])).is_err(), "a search needs words");
        assert!(valid(&skill(Q, &[("system", "")])).is_err(), "one step isn't a skill");
        assert!(valid(&skill("the and what", &[("system", ""), ("memory", "")])).is_err(), "it needs words that call for it");
        assert!(valid(&Skill { name: "x".repeat(61), ..good.clone() }).is_err());
    }

    #[test]
    fn skills_are_files_numbered_when_the_name_is_taken() {
        let dir = std::env::temp_dir().join(format!("kith-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: no other test reads XDG_CONFIG_HOME.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        let one = skill(Q, &[("system", ""), ("memory", "")]);
        let first = save(&one).unwrap();
        let second = save(&one).unwrap();
        assert!(first.ends_with("autark-ai/skills/how-much-disk-is-left-and-what-s-the-lat.toml"), "{first:?}");
        assert!(second.to_string_lossy().ends_with("-2.toml"));
        assert_eq!(all(), vec![one.clone(), one]);
        std::fs::write(folder().join("broken.toml"), "name = 1").unwrap();
        std::fs::write(folder().join("actions.toml"), "name = \"x\"\nwhen = \"mute the volume now\"\n[[steps]]\ntool = \"run_actions\"\n[[steps]]\ntool = \"system\"\n").unwrap();
        assert_eq!(all().len(), 2, "files that don't read, or hold a tool a skill can't use, are left out");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_long_question_is_named_at_a_word() {
        let name = name_of("could you tell me how much disk is left on this laptop and also what the latest linux kernel news is today?");
        assert!(name.chars().count() <= 50 && name.starts_with("Could you tell me") && !name.ends_with(' '), "{name}");
    }
}
