//! Desktop entries (`.desktop` files): finding an app's display name from its Wayland app ID.

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
}

fn read(path: &std::path::Path) -> Option<Entry> {
    if path.extension()? != "desktop" {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut entry = Entry { name: None, wm_class: None };
    let mut in_main_group = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_main_group = line == "[Desktop Entry]";
        } else if in_main_group {
            if let Some(name) = line.strip_prefix("Name=") {
                entry.name = Some(name.trim().to_owned());
            } else if let Some(class) = line.strip_prefix("StartupWMClass=") {
                entry.wm_class = Some(class.trim().to_owned());
            }
        }
    }
    Some(entry)
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
