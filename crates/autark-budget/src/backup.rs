//! A copy of everything Budget keeps (your settings and the logs) in a folder you can put anywhere, and the way back.
//! The functions take their folders as arguments so a test never touches your own.

use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

/// The logs, in the data folder. The settings are `budget.toml`, kept under the same name in the copy.
const DATA_FILES: [&str; 4] = ["readings.jsonl", "bills.jsonl", "payments.jsonl", "spending.jsonl"];
const SETTINGS: &str = "budget.toml";

/// Where `backup` puts a copy when you name no folder: one folder a day in `~/Backups/autark-budget`.
pub fn default_folder(today: NaiveDate) -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Backups").join("autark-budget").join(today.to_string()))
}

/// Copies the settings and every log that exists into `to`. Returns the names copied.
pub fn back_up(to: &Path, data: &Path, config: &Path) -> io::Result<Vec<String>> {
    let wanted = DATA_FILES.iter().map(|name| (data.join(name), *name)).chain([(config.to_path_buf(), SETTINGS)]);
    let present: Vec<(PathBuf, &str)> = wanted.filter(|(path, _)| path.is_file()).collect();
    if present.is_empty() {
        return Err(io::Error::other("there is nothing to back up yet"));
    }
    std::fs::create_dir_all(to)?;
    let mut copied = Vec::new();
    for (path, name) in present {
        std::fs::copy(path, to.join(name))?;
        copied.push(name.to_string());
    }
    Ok(copied)
}

/// Puts a copy made by `back_up` back. What is there now is first copied to `before-restore` in the data folder, so a
/// restore can be taken back. Files the copy does not have are left as they are.
pub fn restore(from: &Path, data: &Path, config: &Path) -> Result<Vec<String>, String> {
    let found: Vec<&str> = DATA_FILES.iter().copied().chain([SETTINGS]).filter(|name| from.join(name).is_file()).collect();
    if found.is_empty() {
        return Err(format!("{} holds no Budget backup (no budget.toml or .jsonl files)", from.display()));
    }
    let safety = data.join("before-restore");
    // Nothing to keep when nothing is there yet.
    match back_up(&safety, data, config) {
        Ok(_) => {}
        Err(error) if error.to_string().contains("nothing to back up") => {}
        Err(error) => return Err(format!("could not keep what is there now: {error}")),
    }
    std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
    for name in &found {
        let target = if *name == SETTINGS { config.to_path_buf() } else { data.join(name) };
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::copy(from.join(name), &target).map_err(|e| format!("could not restore {name}: {e}"))?;
    }
    Ok(found.iter().map(|name| name.to_string()).collect())
}

/// `backup [folder]` and `restore <folder>` from the terminal.
pub fn command(args: &[&str], data: &Path, config: &Path, today: NaiveDate) -> Result<String, String> {
    match args {
        ["backup"] | ["backup", _] => {
            let to = match args.get(1) {
                Some(folder) => PathBuf::from(folder),
                None => default_folder(today).ok_or("no home folder: name one, as in autark-budget backup <folder>")?,
            };
            let copied = back_up(&to, data, config).map_err(|e| format!("could not back up: {e}"))?;
            Ok(format!("Backed up {} to {}.\nPut it back with: autark-budget restore {}", copied.join(", "), to.display(), to.display()))
        }
        ["restore", folder] => {
            let put = restore(Path::new(folder), data, config)?;
            Ok(format!("Restored {}.\nWhat was there before is in {}.", put.join(", "), data.join("before-restore").display()))
        }
        _ => Err("usage: autark-budget backup [folder] | autark-budget restore <folder>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("autark-budget-backup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_backup_holds_the_settings_and_every_log_that_exists() {
        let s = Scratch::new("copy");
        let (data, config, to) = (s.0.join("data"), s.0.join("config/budget.toml"), s.0.join("copy"));
        write(&data.join("readings.jsonl"), "r\n");
        write(&data.join("spending.jsonl"), "s\n");
        write(&config, "[finance]\n");
        let copied = back_up(&to, &data, &config).unwrap();
        assert_eq!(copied, ["readings.jsonl", "spending.jsonl", "budget.toml"]);
        assert_eq!(std::fs::read_to_string(to.join("budget.toml")).unwrap(), "[finance]\n");
        assert!(!to.join("bills.jsonl").exists());
    }

    #[test]
    fn backing_up_nothing_is_an_error_not_an_empty_folder() {
        let s = Scratch::new("empty");
        let error = back_up(&s.0.join("copy"), &s.0.join("data"), &s.0.join("none.toml")).unwrap_err();
        assert!(error.to_string().contains("nothing to back up"));
        assert!(!s.0.join("copy").exists());
    }

    #[test]
    fn restoring_puts_files_back_and_keeps_what_was_there() {
        let s = Scratch::new("restore");
        let (data, config, to) = (s.0.join("data"), s.0.join("config/budget.toml"), s.0.join("copy"));
        write(&data.join("payments.jsonl"), "old\n");
        write(&config, "old = 1\n");
        back_up(&to, &data, &config).unwrap();
        write(&data.join("payments.jsonl"), "new\n");
        write(&config, "new = 2\n");
        write(&data.join("bills.jsonl"), "kept\n");

        let put = restore(&to, &data, &config).unwrap();
        assert_eq!(put, ["payments.jsonl", "budget.toml"]);
        assert_eq!(std::fs::read_to_string(data.join("payments.jsonl")).unwrap(), "old\n");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "old = 1\n");
        assert_eq!(std::fs::read_to_string(data.join("bills.jsonl")).unwrap(), "kept\n", "a file the backup lacks is left alone");
        assert_eq!(std::fs::read_to_string(data.join("before-restore/payments.jsonl")).unwrap(), "new\n");
        assert_eq!(std::fs::read_to_string(data.join("before-restore/budget.toml")).unwrap(), "new = 2\n");
    }

    #[test]
    fn restoring_from_a_folder_that_is_no_backup_is_refused() {
        let s = Scratch::new("refuse");
        write(&s.0.join("stuff/notes.txt"), "x");
        let error = restore(&s.0.join("stuff"), &s.0.join("data"), &s.0.join("budget.toml")).unwrap_err();
        assert!(error.contains("no Budget backup"));
        assert!(!s.0.join("data").exists(), "nothing was touched");
    }
}
