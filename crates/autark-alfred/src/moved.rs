//! The daemon was `autark-agentd` before it was ALFRED. A team made then is moved, once, to the new folders:
//! `~/.config/autark-agentd` to `~/.config/autark-alfred` and `~/.local/state/autark-agentd` to
//! `~/.local/state/autark-alfred`. An unrelated program has used the name `autark-agentd` too, so a folder is moved only
//! when it holds what this daemon wrote: agents (`agents/<id>/agent.toml`) in the config folder; in the state folder, its
//! own event log or a conversation (`agents/<id>/chat.json`), or any state folder when the team itself was just moved.
//! Nothing is moved when the new folder is there already.

use std::path::{Path, PathBuf};

const OLD: &str = "autark-agentd";
const NEW: &str = "autark-alfred";

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Moves the old folders, as above. Called once the bus name is ours and before the starter team is made.
pub fn migrate() {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"));
    migrate_in(&config, &state);
}

fn migrate_in(config: &Path, state: &Path) {
    let team = move_if(config, |old| holds(&old.join("agents"), "agent.toml"));
    // A quiet fortnight can leave the log empty or gone, so the team moving, or a conversation, is proof enough too.
    move_if(state, |old| team || holds_events(&old.join("events.jsonl")) || holds(&old.join("agents"), "chat.json"));
}

/// Moves `base`'s old folder to the new one when the new one isn't there and `ours` says the old one is this daemon's.
/// Returns whether it was moved.
fn move_if(base: &Path, ours: impl Fn(&Path) -> bool) -> bool {
    let (old, new) = (base.join(OLD), base.join(NEW));
    if new.symlink_metadata().is_ok() || !ours(&old) {
        return false;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => {
            eprintln!("alfred: moved {} to {}", old.display(), new.display());
            true
        }
        Err(err) => {
            eprintln!("alfred: moving {} to {}: {err}", old.display(), new.display());
            false
        }
    }
}

/// Whether `agents` holds at least one agent's folder with `file` in it (`agent.toml` or `chat.json`).
fn holds(agents: &Path, file: &str) -> bool {
    std::fs::read_dir(agents).into_iter().flatten().flatten().any(|entry| entry.path().join(file).is_file())
}

/// Whether `path` is this daemon's event log: its first line an event with a `seq` and a `kind`.
fn holds_events(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let Some(line) = text.lines().find(|line| !line.trim().is_empty()) else { return false };
    serde_json::from_str::<serde_json::Value>(line).is_ok_and(|event| event["seq"].is_u64() && event["kind"].is_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alfred-moved-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        dir
    }

    fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const EVENT: &str = "{\"seq\":1,\"time\":1,\"kind\":\"said\",\"agent\":\"atlas\",\"text\":\"hi\"}\n";

    #[test]
    fn a_team_in_the_old_folders_is_moved() {
        let dir = base("old");
        let (config, state) = (dir.join("config"), dir.join("state"));
        put(&config.join("autark-agentd/agents/atlas/agent.toml"), "name = \"ATLAS\"\n");
        put(&config.join("autark-agentd/agents/.seeded"), "");
        put(&state.join("autark-agentd/events.jsonl"), EVENT);
        put(&state.join("autark-agentd/agents/atlas/chat.json"), "{}");
        migrate_in(&config, &state);
        assert!(!config.join("autark-agentd").exists() && !state.join("autark-agentd").exists());
        assert_eq!(std::fs::read_to_string(config.join("autark-alfred/agents/atlas/agent.toml")).unwrap(), "name = \"ATLAS\"\n");
        assert!(config.join("autark-alfred/agents/.seeded").is_file());
        assert_eq!(std::fs::read_to_string(state.join("autark-alfred/events.jsonl")).unwrap(), EVENT);
        assert!(state.join("autark-alfred/agents/atlas/chat.json").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_state_goes_with_the_team_when_its_log_is_empty_or_gone() {
        for (test, log) in [("empty", Some("")), ("gone", None)] {
            let dir = base(test);
            let (config, state) = (dir.join("config"), dir.join("state"));
            put(&config.join("autark-agentd/agents/atlas/agent.toml"), "name = \"ATLAS\"\n");
            put(&state.join("autark-agentd/agents/atlas/chat.json"), "{}");
            if let Some(log) = log {
                put(&state.join("autark-agentd/events.jsonl"), log);
            }
            migrate_in(&config, &state);
            assert!(config.join("autark-alfred/agents/atlas/agent.toml").is_file(), "{test}");
            assert!(state.join("autark-alfred/agents/atlas/chat.json").is_file() && !state.join("autark-agentd").exists(), "{test}");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn chats_alone_are_proof_enough_for_the_state() {
        let dir = base("chats");
        let (config, state) = (dir.join("config"), dir.join("state"));
        // The team moved on an earlier start, but the state folder couldn't be then.
        put(&config.join("autark-alfred/agents/atlas/agent.toml"), "name = \"ATLAS\"\n");
        put(&state.join("autark-agentd/agents/atlas/chat.json"), "{}");
        migrate_in(&config, &state);
        assert!(state.join("autark-alfred/agents/atlas/chat.json").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn new_folders_are_left_as_they_are() {
        let dir = base("new");
        let (config, state) = (dir.join("config"), dir.join("state"));
        put(&config.join("autark-agentd/agents/atlas/agent.toml"), "name = \"ATLAS\"\n");
        put(&state.join("autark-agentd/events.jsonl"), EVENT);
        put(&config.join("autark-alfred/agents/piper/agent.toml"), "name = \"PIPER\"\n");
        // Even an empty new folder is the new one: it is never replaced.
        std::fs::create_dir_all(state.join("autark-alfred")).unwrap();
        migrate_in(&config, &state);
        assert!(config.join("autark-agentd/agents/atlas/agent.toml").is_file() && state.join("autark-agentd/events.jsonl").is_file());
        assert!(!config.join("autark-alfred/agents/atlas").exists());
        assert_eq!(std::fs::read_dir(state.join("autark-alfred")).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn another_programs_folders_are_not_moved() {
        let dir = base("other");
        let (config, state) = (dir.join("config"), dir.join("state"));
        // Folders of the same name that this daemon didn't write: no agents, and a log of another shape.
        put(&config.join("autark-agentd/settings.toml"), "x = 1\n");
        put(&config.join("autark-agentd/agents/readme.txt"), "");
        put(&state.join("autark-agentd/events.jsonl"), "{\"session\":\"s1\",\"tokens\":3}\n");
        put(&state.join("autark-agentd/usage.json"), "{}");
        migrate_in(&config, &state);
        assert!(config.join("autark-agentd/settings.toml").is_file() && state.join("autark-agentd/usage.json").is_file());
        assert!(!config.join("autark-alfred").exists() && !state.join("autark-alfred").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
