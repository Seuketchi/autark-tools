//! `launcher.toml`: what a search shows. Every field is optional, and what is left out is as it shipped.

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Launcher {
    /// The most apps one search lists, from 1 to 12.
    pub max_apps: usize,
    /// The most recent files one search lists, from 0 to 10.
    pub max_files: usize,
    /// Whether the system actions (lock, sleep, restart, shut down, log out) are found.
    pub actions: bool,
}

impl Default for Launcher {
    fn default() -> Self {
        Self { max_apps: 6, max_files: 4, actions: true }
    }
}

impl Launcher {
    /// `launcher.toml` as it is now, with the numbers kept in range.
    pub fn load() -> Self {
        Self::clamped(autark_common::config::load("launcher"))
    }

    fn clamped(mut self) -> Self {
        self.max_apps = self.max_apps.clamp(1, 12);
        self.max_files = self.max_files.min(10);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_set_is_the_launcher_that_shipped() {
        assert_eq!(Launcher::default(), Launcher { max_apps: 6, max_files: 4, actions: true });
        assert_eq!(toml::from_str::<Launcher>("").unwrap(), Launcher::default());
    }

    #[test]
    fn a_file_changes_only_what_it_names_and_numbers_stay_in_range() {
        let launcher = toml::from_str::<Launcher>("max_apps = 99\nmax_files = 0\n").unwrap().clamped();
        assert_eq!(launcher, Launcher { max_apps: 12, max_files: 0, actions: true });
        assert_eq!(toml::from_str::<Launcher>("max_apps = 0").unwrap().clamped().max_apps, 1);
        assert!(toml::from_str::<Launcher>("max_aps = 3").is_err(), "a misspelt key is an error, so the typo is seen");
    }
}
