//! What the launcher finds: apps by name, recent files, and system actions, ranked with how
//! often each app was opened from here, then, with AI on, a row that asks Kith.

use std::collections::HashMap;
use std::path::PathBuf;

use gtk::gio;
use gtk::prelude::*;

use crate::actions::{self, Action};

#[derive(Clone, Copy, PartialEq)]
pub enum Group {
    Applications,
    Files,
    Actions,
    Kith,
}

impl Group {
    pub fn heading(self) -> &'static str {
        match self {
            Group::Applications => "APPLICATIONS",
            Group::Files => "FILES",
            Group::Actions => "ACTIONS",
            Group::Kith => "KITH",
        }
    }
}

pub enum Target {
    App(gio::AppInfo),
    File(String),
    Action(Action),
    /// Ask Kith about what's typed.
    Kith(String),
}

pub enum Icon {
    Gicon(gio::Icon),
    /// A symbolic icon shown on a grey tile.
    Glyph(&'static str),
    /// Kith's sparkle.
    Kith,
}

pub struct Hit {
    pub group: Group,
    pub title: String,
    pub detail: String,
    pub icon: Icon,
    pub target: Target,
}

const MAX_APPS: usize = 6;
const MAX_FILES: usize = 4;

pub struct Index {
    apps: Vec<gio::AppInfo>,
    /// Opens per app ID, kept in `$XDG_STATE_HOME/autark/launcher-history`.
    history: HashMap<String, u32>,
}

impl Index {
    pub fn load() -> Self {
        let history = std::fs::read_to_string(history_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let (count, id) = line.split_once('\t')?;
                Some((id.to_owned(), count.parse().ok()?))
            })
            .collect();
        Self { apps: Vec::new(), history }
    }

    /// Re-reads installed apps; cheap enough to do each time the launcher opens.
    pub fn refresh(&mut self) {
        self.apps = gio::AppInfo::all().into_iter().filter(|app| app.should_show()).collect();
    }

    pub fn opened(&mut self, app: &gio::AppInfo) {
        let Some(id) = app.id() else {
            return;
        };
        *self.history.entry(id.to_string()).or_default() += 1;
        let text: String = self.history.iter().map(|(id, count)| format!("{count}\t{id}\n")).collect();
        let path = history_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(err) = std::fs::write(&path, text) {
            eprintln!("launcher: {}: {err}", path.display());
        }
    }

    /// What matches `query`, ending with a row that asks Kith about it when `kith` is on.
    pub fn search(&self, query: &str, kith: bool) -> Vec<Hit> {
        let typed = query.trim();
        let query = typed.to_lowercase();
        let mut hits = self.apps(&query);
        // One letter matches too much outside app names.
        if query.chars().count() >= 2 {
            hits.extend(files(&query));
            hits.extend(actions(&query));
        }
        if kith && !typed.is_empty() {
            hits.push(Hit {
                group: Group::Kith,
                title: format!("Ask Kith: {typed}"),
                detail: "Do it or answer it, from what you typed".to_owned(),
                icon: Icon::Kith,
                target: Target::Kith(typed.to_owned()),
            });
        }
        hits
    }

    /// Apps whose name starts with, has a word starting with, or contains the query; with no
    /// query, the ones opened most.
    fn apps(&self, query: &str) -> Vec<Hit> {
        let mut scored: Vec<(u32, u32, &gio::AppInfo)> = self
            .apps
            .iter()
            .filter_map(|app| {
                let name = app.display_name().to_lowercase();
                let score = if query.is_empty() {
                    1
                } else if name.starts_with(query) {
                    4
                } else if name.split_whitespace().any(|word| word.starts_with(query)) {
                    3
                } else if name.contains(query) {
                    2
                } else if app.description().is_some_and(|text| text.to_lowercase().contains(query))
                    || app.executable().to_string_lossy().to_lowercase().contains(query)
                {
                    1
                } else {
                    return None;
                };
                let opened = app.id().and_then(|id| self.history.get(id.as_str()).copied()).unwrap_or(0);
                (!query.is_empty() || opened > 0).then_some((score, opened, app))
            })
            .collect();
        scored.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)).then_with(|| a.2.display_name().cmp(&b.2.display_name())));
        scored
            .into_iter()
            .take(MAX_APPS)
            .map(|(_, _, app)| Hit {
                group: Group::Applications,
                title: app.display_name().to_string(),
                detail: "Application".to_owned(),
                icon: app
                    .icon()
                    .map(Icon::Gicon)
                    .unwrap_or(Icon::Glyph("application-x-executable-symbolic")),
                target: Target::App(app.clone()),
            })
            .collect()
    }
}

/// Recently used files whose name contains the query.
fn files(query: &str) -> Vec<Hit> {
    let home = std::env::var("HOME").unwrap_or_default();
    gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|item| item.exists() && item.display_name().to_lowercase().contains(query))
        .take(MAX_FILES)
        .map(|item| {
            let uri = item.uri().to_string();
            let folder = gio::File::for_uri(&uri)
                .parent()
                .and_then(|parent| parent.path())
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            let detail = match folder.strip_prefix(&home) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => folder,
            };
            Hit {
                group: Group::Files,
                title: item.display_name().to_string(),
                detail,
                icon: Icon::Gicon(item.gicon().unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast())),
                target: Target::File(uri),
            }
        })
        .collect()
}

fn actions(query: &str) -> Vec<Hit> {
    actions::ALL
        .into_iter()
        .filter(|action| {
            action.title().to_lowercase().contains(query) || action.keywords().iter().any(|word| word.contains(query))
        })
        .map(|action| Hit {
            group: Group::Actions,
            title: action.title().to_owned(),
            detail: action.detail().to_owned(),
            icon: Icon::Glyph(action.icon()),
            target: Target::Action(action),
        })
        .collect()
}

fn history_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_default()
        .join("autark/launcher-history")
}
