//! What Kith can see, by what a question is about. Code reads the facts and gives them to the model as context, so the
//! answer comes from them. A subject Kith has no source for says so in the context: asked about a calendar it can't
//! see, a small model makes one up, and a rule in the system prompt did not stop it.

use std::process::Command;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Subject {
    /// Disk, battery, memory, uptime: read from this laptop.
    System,
    Calendar,
    Mail,
    Tasks,
    /// News and look-ups: a web search, when it is on.
    Web,
    /// What the user said to Kith before, and what is in their notes.
    Recall,
}

impl Subject {
    /// What an agent's `can` list calls it.
    pub fn ability(self) -> &'static str {
        match self {
            Subject::System => "laptop",
            Subject::Calendar => "calendar",
            Subject::Mail => "mail",
            Subject::Tasks => "tasks",
            Subject::Web => "web",
            Subject::Recall => "notes",
        }
    }
}

/// What the question is about.
pub fn subjects(question: &str) -> Vec<Subject> {
    let question = question.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| question.contains(word));
    let mut found = Vec::new();
    if has(&["disk", "storage", "battery", "uptime", "how much ram", "how much memory", "free memory", "memory left"]) {
        found.push(Subject::System);
    }
    if has(&["calendar", "schedule", "meeting", "appointment", "agenda"]) {
        found.push(Subject::Calendar);
    }
    if has(&["email", "e-mail", "inbox", "unread", "gmail", "my mail", "new mail"]) {
        found.push(Subject::Mail);
    }
    if has(&["my task", "to-do", "todo", "to do list", "task for today", "tasks for today", "my project", "project board", "github project", "what should i work on", "what should i do today"]) {
        found.push(Subject::Tasks);
    }
    if has(&["news", "latest", "look up", "lookup", "search for", "search the web", "what happened"]) {
        found.push(Subject::Web);
    }
    if has(&["my notes", "in my notes", "we discussed", "we talked", "we decided", "i told you", "you told me", "did i say", "did i tell", "remember when", "last week", "what did i"]) {
        found.push(Subject::Recall);
    }
    found
}

/// What to tell the model about a subject Kith has no source for.
pub fn unavailable(subject: Subject) -> &'static str {
    match subject {
        Subject::Calendar => "Kith cannot see the user's calendar yet: no calendar address has been given to Kith. Tell the user that in one sentence. Do not make up events.",
        Subject::Mail => "Kith cannot read the user's email just now: no mail account is connected, or the mail service did not answer. Tell the user that in one sentence. Do not make up messages.",
        Subject::Tasks => "Kith cannot see the user's tasks just now: the GitHub projects could not be read. Tell the user that in one sentence. Do not make up tasks.",
        Subject::Web => "Kith cannot search the internet right now: web search is off or not set up. Tell the user that in one sentence. Do not make up news or links.",
        Subject::System | Subject::Recall => "",
    }
}

/// Facts read from this laptop just now.
pub fn system() -> String {
    let mut lines = vec!["Facts read from this laptop just now (answer from these, with these numbers):".to_owned()];
    if let Ok(output) = Command::new("df").args(["-P", "-B1", "/", "/home"]).output() {
        lines.extend(disks(&String::from_utf8_lossy(&output.stdout)));
    }
    lines.extend(battery());
    lines.extend(memory(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default()));
    lines.extend(uptime(&std::fs::read_to_string("/proc/uptime").unwrap_or_default()));
    lines.join("\n")
}

/// Bytes as the `df -h` the user knows: binary units, labelled G and M.
fn size(bytes: f64) -> String {
    if bytes >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", bytes / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{:.0} MB", bytes / (1024.0 * 1024.0))
    }
}

/// One line per disk in `df -P -B1` output.
fn disks(df: &str) -> Vec<String> {
    df.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (total, used, free) = (fields.get(1)?.parse::<f64>().ok()?, fields.get(2)?.parse::<f64>().ok()?, fields.get(3)?.parse::<f64>().ok()?);
            let mount = fields.last()?;
            let percent = if total > 0.0 { (used / total * 100.0).round() } else { 0.0 };
            Some(format!("- Disk mounted at {mount}: {percent}% used, {} free of {}", size(free), size(total)))
        })
        .collect()
}

fn battery() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let read = |name: &str| std::fs::read_to_string(entry.path().join(name)).map(|text| text.trim().to_owned()).unwrap_or_default();
        if read("type") == "Battery" {
            found.push(format!("- Battery: {}%, {}", read("capacity"), read("status").to_lowercase()));
        }
    }
    found.sort();
    found
}

fn memory(meminfo: &str) -> Option<String> {
    let kb = |name: &str| meminfo.lines().find(|line| line.starts_with(name))?.split_whitespace().nth(1)?.parse::<f64>().ok();
    let (total, available) = (kb("MemTotal:")?, kb("MemAvailable:")?);
    Some(format!("- Memory: {} available of {}", size(available * 1024.0), size(total * 1024.0)))
}

fn uptime(proc_uptime: &str) -> Option<String> {
    let seconds = proc_uptime.split_whitespace().next()?.parse::<f64>().ok()? as u64;
    Some(format!("- Up for {} h {} min", seconds / 3600, seconds % 3600 / 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_is_sorted_by_its_subject() {
        assert_eq!(subjects("How much disk space do I have left?"), vec![Subject::System]);
        assert_eq!(subjects("What is on my calendar today?"), vec![Subject::Calendar]);
        assert_eq!(subjects("whats my task for today"), vec![Subject::Tasks]);
        assert_eq!(subjects("Any unread email? And the latest news on Linux?"), vec![Subject::Mail, Subject::Web]);
        assert!(subjects("Why is the sky blue?").is_empty());
    }

    #[test]
    fn disks_are_read_from_df() {
        let df = "Filesystem     1-blocks        Used   Available Capacity Mounted on\n\
                  /dev/mapper/autark 42949672960 20401094656 20401094656      50% /\n\
                  /dev/mapper/autark-home 9448928051 8719728640 613416960      94% /home\n";
        assert_eq!(disks(df), vec!["- Disk mounted at /: 48% used, 19.0 GB free of 40.0 GB", "- Disk mounted at /home: 92% used, 585 MB free of 8.8 GB"]);
        assert!(disks("Filesystem\n").is_empty());
    }

    #[test]
    fn memory_and_uptime_are_read_from_proc() {
        assert_eq!(memory("MemTotal:       16777216 kB\nMemFree: 1 kB\nMemAvailable:    8388608 kB\n").unwrap(), "- Memory: 8.0 GB available of 16.0 GB");
        assert_eq!(uptime("7384.52 28000.1\n").unwrap(), "- Up for 2 h 3 min");
        assert!(memory("").is_none() && uptime("").is_none());
    }

    #[test]
    fn what_cannot_be_seen_says_so() {
        for subject in [Subject::Calendar, Subject::Mail, Subject::Tasks, Subject::Web] {
            assert!(unavailable(subject).contains("Do not make up"));
        }
    }
}
