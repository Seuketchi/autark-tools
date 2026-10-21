//! Desktop entries (`.desktop` files): finding an app's display name from its Wayland app ID, and an app by
//! the name you'd call it, with the command that starts it.

use std::path::PathBuf;

/// The `Name` of the app whose desktop entry matches `app_id`, by file name first and then by
/// `StartupWMClass`.
pub fn app_name(app_id: &str) -> Option<String> {
    find(app_id).and_then(|(_, entry)| entry.name)
}

/// The desktop file name (such as `firefox.desktop`) for a Wayland app ID, matched the same way.
pub fn desktop_id(app_id: &str) -> Option<String> {
    let (path, _) = find(app_id)?;
    Some(path.file_name()?.to_str()?.to_owned())
}

fn find(app_id: &str) -> Option<(PathBuf, Entry)> {
    let dirs = application_dirs();
    let file = format!("{app_id}.desktop");
    let lower = file.to_lowercase();
    for dir in &dirs {
        for candidate in [&file, &lower] {
            let path = dir.join(candidate);
            if let Some(entry) = read(&path) {
                return Some((path, entry));
            }
        }
    }
    // Some apps' IDs don't match their file name, so look for one that claims the ID.
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            Some((path.clone(), read(&path)?))
        })
        .find(|(_, entry)| entry.wm_class.as_deref().is_some_and(|class| class.eq_ignore_ascii_case(app_id)))
}

struct Entry {
    name: Option<String>,
    wm_class: Option<String>,
    exec: Option<String>,
    /// NoDisplay, Hidden, or not an application: not one to open by name.
    hidden: bool,
}

fn read(path: &std::path::Path) -> Option<Entry> {
    if path.extension()? != "desktop" {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut entry = Entry { name: None, wm_class: None, exec: None, hidden: false };
    let mut in_main_group = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_main_group = line == "[Desktop Entry]";
        } else if in_main_group {
            if let Some(name) = line.strip_prefix("Name=") {
                entry.name = Some(name.trim().to_owned());
            } else if let Some(class) = line.strip_prefix("StartupWMClass=") {
                entry.wm_class = Some(class.trim().to_owned());
            } else if let Some(exec) = line.strip_prefix("Exec=") {
                entry.exec = Some(exec.trim().to_owned());
            } else if line == "NoDisplay=true" || line == "Hidden=true" || line.strip_prefix("Type=").is_some_and(|kind| kind.trim() != "Application") {
                entry.hidden = true;
            }
        }
    }
    Some(entry)
}

/// Every app that can be opened from a menu, as `(file name without .desktop, display name)`, by display name. Where two
/// folders have a file of the same name, the first folder's wins, as it overrides the others.
pub fn installed() -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for dir in application_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for file in entries.flatten() {
            let id = file.file_name().to_string_lossy().into_owned();
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(entry) = read(&file.path()) else { continue };
            if let (Some(name), Some(_), false) = (entry.name, entry.exec, entry.hidden) {
                apps.push((id.trim_end_matches(".desktop").to_owned(), name));
            }
        }
    }
    apps.sort_by_key(|(id, name)| (name.to_lowercase(), id.clone()));
    apps
}

/// An app found by name, and the command that starts it.
#[derive(Debug, PartialEq)]
pub struct App {
    /// Its desktop file name, such as `org.gnome.Nautilus.desktop`.
    pub id: String,
    pub name: String,
    pub command: Vec<String>,
}

/// The app you'd call `name`: its name exactly (ignoring case), else its file name, else the shortest
/// name that starts with it, else one that contains it. Apps hidden from menus aren't found.
pub fn find_app(name: &str) -> Option<App> {
    let wanted = name.trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut best: Option<(u8, App)> = None;
    for dir in application_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for file in entries.flatten() {
            let id = file.file_name().to_string_lossy().into_owned();
            // The first directory with a file of this name wins, as it overrides the others.
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(entry) = read(&file.path()) else { continue };
            let (Some(app_name), Some(exec)) = (entry.name, entry.exec) else { continue };
            if entry.hidden {
                continue;
            }
            let lower = app_name.to_lowercase();
            let stem = id.trim_end_matches(".desktop").to_lowercase();
            let rank = if lower == wanted {
                0
            } else if stem == wanted || stem.rsplit('.').next() == Some(wanted.as_str()) {
                1
            } else if lower.starts_with(&wanted) {
                2
            } else if lower.contains(&wanted) {
                3
            } else {
                continue;
            };
            let command = command(&exec);
            if command.is_empty() {
                continue;
            }
            let better = best.as_ref().is_none_or(|(old, found)| rank < *old || (rank == *old && app_name.len() < found.name.len()));
            if better {
                best = Some((rank, App { id, name: app_name, command }));
            }
        }
    }
    best.map(|(_, app)| app)
}

/// An `Exec` line as program and arguments, with its field codes (`%U`, `%f` and the rest) left out, since
/// the app is opened with nothing to hand it.
pub fn command(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            // A field code is dropped, and so is the word if it was only that; "%%" is a percent sign.
            '%' => {
                if chars.next() == Some('%') {
                    word.push('%');
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            c => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// `applications/` under `$XDG_DATA_HOME` and each of `$XDG_DATA_DIRS`, most specific first.
fn application_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    home.into_iter()
        .chain(system.split(':').map(PathBuf::from))
        .map(|dir| dir.join("applications"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::command;

    #[test]
    fn exec_lines_lose_their_field_codes() {
        assert_eq!(command("firefox %u"), vec!["firefox"]);
        assert_eq!(command("env GDK_BACKEND=wayland foot --title \"My term\" %F"), vec!["env", "GDK_BACKEND=wayland", "foot", "--title", "My term"]);
        assert_eq!(command("app --rate=50%% \"a \\\"b\\\"\""), vec!["app", "--rate=50%", "a \"b\""]);
    }
}
