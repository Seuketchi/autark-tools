//! What Kith can see of the laptop, by what a question is about. Code reads the facts and gives them to the model as
//! context, so the answer comes from them: disk, battery, memory and uptime, and how the last `autark-pkg` build went.

use std::path::Path;
use std::process::Command;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Subject {
    /// Disk, battery, memory, uptime: read from this laptop.
    System,
    /// The last `autark-pkg` build, read from its log.
    Build,
}

/// What the question is about.
pub fn subjects(question: &str) -> Vec<Subject> {
    let question = question.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| question.contains(word));
    let mut found = Vec::new();
    if has(&["disk", "storage", "battery", "uptime", "how much ram", "how much memory", "free memory", "memory left"]) {
        found.push(Subject::System);
    }
    if has(&["build", "compile", "autark-pkg"]) {
        found.push(Subject::Build);
    }
    found
}

/// How the newest build log in `folder` says its build went, with the lines before a failure; or that there is none.
pub fn build(folder: &Path) -> String {
    let newest = std::fs::read_dir(folder).ok().and_then(|entries| {
        entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "log"))
            .filter_map(|entry| Some((std::fs::symlink_metadata(entry.path()).ok().filter(std::fs::Metadata::is_file)?.modified().ok()?, entry.path())))
            .max()
    });
    let Some((_, path)) = newest else {
        return "There are no autark-pkg build logs on this laptop. Say so in one sentence; do not make up a build.".to_owned();
    };
    match crate::events::build_ended(&path) {
        Some(event) => format!("The last autark-pkg build, read from its log just now (answer from this):\n{}", event.facts),
        None => format!("The last autark-pkg build's log, {}, has no outcome line yet: the build is still running or was cut off.", path.display()),
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
        assert_eq!(subjects("Why did my last build fail? Battery?"), vec![Subject::System, Subject::Build]);
        assert!(subjects("What is on my calendar today?").is_empty());
        assert!(subjects("Why is the sky blue?").is_empty());
    }

    #[test]
    fn the_last_build_is_read_from_the_newest_log() {
        let folder = std::env::temp_dir().join(format!("autark-ai-senses-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        assert!(build(&folder).contains("no autark-pkg build logs"));
        std::fs::write(folder.join("old.log"), "== autark-pkg: old 1.0 built in 3s\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(folder.join("gcc.log"), "make\n== autark-pkg: gcc 15.2.0 built in 812s\n").unwrap();
        assert!(build(&folder).ends_with("An autark-pkg build ended: gcc 15.2.0 built in 812s."));
        std::fs::remove_dir_all(&folder).unwrap();
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
}
