//! Per-app profiles: `/etc/autark/sandbox/<app>.toml`. An app without one doesn't run.
//!
//! ```toml
//! description = "Zoom"
//! exec = "/opt/zoom/zoom"          # its folder is readable automatically
//! args = []
//! read = ["~/Pictures"]            # read and execute
//! write = ["~/Downloads", "~/.zoom"]
//! network = "all"                  # "none" (the default), or { connect = [443], bind = [] }
//! wayland = true                   # the default
//! gpu = true                       # /dev/dri; the default
//! audio = true                     # PipeWire (and its PulseAudio socket)
//! dbus = false                     # the whole session bus: systemd on it can start anything outside
//! [env]
//! QT_QPA_PLATFORM = "wayland"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default)]
    pub description: String,
    pub exec: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub network: Network,
    #[serde(default = "yes")]
    pub wayland: bool,
    #[serde(default = "yes")]
    pub gpu: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub dbus: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Network {
    #[default]
    #[serde(skip)]
    Unset,
    Word(String),
    Ports {
        #[serde(default)]
        connect: Vec<u16>,
        #[serde(default)]
        bind: Vec<u16>,
    },
}

/// What the network setting comes to.
#[derive(Debug, PartialEq)]
pub enum Net {
    All,
    /// A network namespace of its own: no interfaces but a downed loopback.
    None,
    /// TCP to and on these ports only (Landlock); UDP isn't covered.
    Ports {
        connect: Vec<u16>,
        bind: Vec<u16>,
    },
}

impl Profile {
    pub fn load(dir: &Path, app: &str) -> Result<Self, String> {
        if app.is_empty() || app.contains('/') || app.starts_with('.') {
            return Err(format!("{app:?} isn't an app name"));
        }
        let path = dir.join(format!("{app}.toml"));
        let text = std::fs::read_to_string(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("{app} has no profile ({}), so it doesn't run", path.display()),
            _ => format!("{}: {error}", path.display()),
        })?;
        let profile: Profile = toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        if !profile.exec.is_absolute() {
            return Err(format!("{}: exec must be an absolute path", path.display()));
        }
        profile.net().map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(profile)
    }

    pub fn net(&self) -> Result<Net, String> {
        match &self.network {
            Network::Unset => Ok(Net::None),
            Network::Word(word) if word == "all" => Ok(Net::All),
            Network::Word(word) if word == "none" => Ok(Net::None),
            Network::Word(word) => Err(format!("network = {word:?} isn't \"all\", \"none\" or a table of ports")),
            Network::Ports { connect, bind } => Ok(Net::Ports { connect: connect.clone(), bind: bind.clone() }),
        }
    }
}

/// `~` and `~/…` under `home`; anything else must be absolute.
pub fn expand(path: &str, home: &Path) -> Result<PathBuf, String> {
    let expanded = if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    };
    if !expanded.is_absolute() {
        return Err(format!("{path}: paths must be absolute or start with ~/"));
    }
    if expanded.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return Err(format!("{path}: no .. in paths"));
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Profile {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn network_defaults_to_none() {
        assert_eq!(parse("exec = \"/opt/a/a\"").net(), Ok(Net::None));
        assert_eq!(parse("exec = \"/opt/a/a\"\nnetwork = \"all\"").net(), Ok(Net::All));
        assert_eq!(parse("exec = \"/opt/a/a\"\nnetwork = { connect = [443] }").net(), Ok(Net::Ports { connect: vec![443], bind: vec![] }));
        assert!(parse("exec = \"/opt/a/a\"\nnetwork = \"some\"").net().is_err());
        let profile = parse("exec = \"/opt/a/a\"");
        assert!(profile.wayland && profile.gpu && !profile.audio && !profile.dbus);
    }

    #[test]
    fn paths_expand_under_home() {
        let home = Path::new("/home/me");
        assert_eq!(expand("~/Downloads", home), Ok(PathBuf::from("/home/me/Downloads")));
        assert_eq!(expand("~", home), Ok(PathBuf::from("/home/me")));
        assert!(expand("Downloads", home).is_err());
        assert!(expand("~/../other", home).is_err());
    }

    #[test]
    fn unknown_fields_and_missing_profiles_are_refused() {
        assert!(toml::from_str::<Profile>("exec = \"/a\"\nwrites = []").is_err());
        let error = Profile::load(Path::new("/nonexistent"), "zoom").unwrap_err();
        assert!(error.contains("has no profile"));
        assert!(Profile::load(Path::new("/etc"), "../passwd").is_err());
    }
}
