//! Where everything lives: `/etc/autark/pkg.toml`, every path absolute on the target system. With `--root`,
//! all of them move under that folder, so a whole system can be tried out in a temporary directory.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Context, Result};

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// The recipes: the autark repo's `scripts/pkg`.
    pub recipes: PathBuf,
    /// Downloaded sources, checked against the recipes' checksums.
    pub sources: PathBuf,
    /// Build folders, removed after a successful build.
    pub builds: PathBuf,
    /// Signed package archives, the last `keep` builds of each package.
    pub cache: PathBuf,
    /// What's installed: one folder per package with its version and file list.
    pub db: PathBuf,
    /// Build logs and the test failures they record.
    pub logs: PathBuf,
    /// Public keys a package may be signed with (minisign `.pub` files).
    pub keys: PathBuf,
    /// The signing key: root's only.
    pub secret_key: PathBuf,
    /// The unprivileged user staged builds run as.
    pub build_user: String,
    pub keep: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recipes: "/usr/src/autark/scripts/pkg".into(),
            sources: "/var/cache/autark/sources".into(),
            builds: "/var/tmp/autark-pkg".into(),
            cache: "/var/cache/autark/pkgs".into(),
            db: "/var/lib/autark/pkg".into(),
            logs: "/var/log/autark-pkg".into(),
            keys: "/etc/autark/pkg-keys".into(),
            secret_key: "/etc/autark/pkg-secret.key".into(),
            build_user: "autark-build".into(),
            keep: 2,
        }
    }
}

/// The settings with every path resolved under the root.
#[derive(Debug)]
pub struct Config {
    pub root: PathBuf,
    pub recipes: PathBuf,
    pub sources: PathBuf,
    pub builds: PathBuf,
    pub cache: PathBuf,
    pub db: PathBuf,
    pub logs: PathBuf,
    pub keys: PathBuf,
    pub secret_key: PathBuf,
    pub build_user: String,
    pub keep: usize,
    /// The paths as the target system sees them, which a tracked build's scan leaves out.
    pub own: Vec<PathBuf>,
}

impl Config {
    /// Reads `file` (default: `<root>/etc/autark/pkg.toml`, optional). `recipes` overrides the setting
    /// and is not moved under the root.
    pub fn load(root: &Path, file: Option<&Path>, recipes: Option<&Path>) -> Result<Self> {
        let default = under(root, Path::new("/etc/autark/pkg.toml"));
        let settings: Settings = match file {
            Some(file) => toml::from_str(&std::fs::read_to_string(file).ctx(file)?).ctx(file)?,
            None => match std::fs::read_to_string(&default) {
                Ok(text) => toml::from_str(&text).ctx(&default)?,
                Err(_) => Settings::default(),
            },
        };
        let own = vec![settings.sources.clone(), settings.builds.clone(), settings.cache.clone(), settings.db.clone(), settings.logs.clone(), settings.recipes.clone()];
        Ok(Self {
            root: root.to_path_buf(),
            recipes: recipes.map_or_else(|| under(root, &settings.recipes), Path::to_path_buf),
            sources: under(root, &settings.sources),
            builds: under(root, &settings.builds),
            cache: under(root, &settings.cache),
            db: under(root, &settings.db),
            logs: under(root, &settings.logs),
            keys: under(root, &settings.keys),
            secret_key: under(root, &settings.secret_key),
            build_user: settings.build_user,
            keep: settings.keep.max(1),
            own,
        })
    }

    /// The real path of `path` (absolute on the target system).
    pub fn path(&self, path: &str) -> PathBuf {
        under(&self.root, Path::new(path))
    }

    /// Whether this is the running system rather than a folder standing in for one.
    pub fn is_live(&self) -> bool {
        self.root == Path::new("/")
    }
}

pub fn under(root: &Path, path: &Path) -> PathBuf {
    root.join(path.strip_prefix("/").unwrap_or(path))
}
