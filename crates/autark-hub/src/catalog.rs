//! What the Hub knows about packages: the JSON `autark-pkg catalog` prints, which of them each view shows, and what
//! state a package is in. The Hub reads and never changes anything itself; changes go through the root helper.

use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub mode: String,
    /// Made from an Arch Linux package by `autark-pkg arch`; nobody on Autark has reviewed it.
    #[serde(default)]
    pub imported: bool,
    pub installed: Option<String>,
    #[serde(default)]
    pub held: bool,
    /// The recipe's version, when `autark-pkg upgrade` would install it.
    pub upgrade: Option<String>,
    /// Builds in the cache, newest first.
    #[serde(default)]
    pub kept: Vec<String>,
}

#[derive(Deserialize)]
struct Catalog {
    packages: Vec<Package>,
}

pub fn parse(json: &str) -> Result<Vec<Package>, String> {
    serde_json::from_str::<Catalog>(json).map(|catalog| catalog.packages).map_err(|error| format!("autark-pkg's catalog wasn't understood: {error}"))
}

/// `autark-pkg catalog`; `AUTARK_PKG` names another program, for trying a build that isn't installed yet.
pub fn load() -> Result<Vec<Package>, String> {
    let program = std::env::var("AUTARK_PKG").unwrap_or_else(|_| "autark-pkg".to_owned());
    let output = Command::new(&program).arg("catalog").stdin(Stdio::null()).output().map_err(|error| format!("couldn't run {program}: {error}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        let first = said.lines().find(|line| !line.trim().is_empty()).unwrap_or("it failed");
        return Err(if said.contains("unknown command") { format!("{program} is too old for the Hub: it has no `catalog` command. Update autark-pkg.") } else { first.to_owned() });
    }
    parse(&String::from_utf8_lossy(&output.stdout))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Browse,
    Installed,
    Updates,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Installed,
    Update(String),
    /// Held at an older build after a rollback; installing it again takes the recipe's version.
    Held,
}

impl Package {
    pub fn state(&self) -> State {
        match (&self.installed, &self.upgrade) {
            (None, _) => State::NotInstalled,
            (Some(_), Some(version)) => State::Update(version.clone()),
            (Some(_), None) if self.held => State::Held,
            (Some(_), None) => State::Installed,
        }
    }

    /// A kept build other than the installed one: what `autark-pkg rollback` goes back to.
    pub fn rollback_to(&self) -> Option<&str> {
        let installed = self.installed.as_deref()?;
        self.kept.iter().map(String::as_str).find(|version| *version != installed)
    }

    /// The same words `autark-pkg search` looks for: every word in the name or the description.
    fn matches(&self, words: &[String]) -> bool {
        let haystack = format!("{} {}", self.name, self.description).to_lowercase();
        words.iter().all(|word| haystack.contains(word.as_str()))
    }
}

/// What a view lists, by name. Browse leaves out packages imported from Arch unless `unreviewed` is on (an installed one
/// always shows, so nothing on the system is hidden); Installed and Updates list what they say.
pub fn visible<'a>(all: &'a [Package], view: View, query: &str, unreviewed: bool) -> Vec<&'a Package> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut list: Vec<&Package> = all
        .iter()
        .filter(|package| match view {
            View::Browse => unreviewed || !package.imported || package.installed.is_some(),
            View::Installed => package.installed.is_some(),
            View::Updates => package.upgrade.is_some(),
        })
        .filter(|package| package.matches(&words))
        .collect();
    list.sort_by_key(|package| package.name.to_lowercase());
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, description: &str) -> Package {
        Package { name: name.into(), version: "1-1".into(), description: description.into(), depends: vec![], mode: "staged".into(), imported: false, installed: None, held: false, upgrade: None, kept: vec![] }
    }

    fn sample() -> Vec<Package> {
        let mut curl = package("curl", "Transfer data with URLs");
        curl.installed = Some("8.0-1".into());
        curl.upgrade = Some("8.1-1".into());
        let mut zlib = package("zlib", "Compression library");
        zlib.installed = Some("1-1".into());
        let mut old = package("Old", "");
        old.installed = Some("1-1".into());
        old.held = true;
        old.kept = vec!["1-1".into(), "0-9".into()];
        let mut arch = package("htop", "Process viewer");
        arch.imported = true;
        let mut arch_installed = package("tree", "Directory listing");
        arch_installed.imported = true;
        arch_installed.installed = Some("1-1".into());
        vec![zlib, curl, old, arch, arch_installed, package("bash", "")]
    }

    fn names(list: Vec<&Package>) -> Vec<&str> {
        list.into_iter().map(|package| package.name.as_str()).collect()
    }

    #[test]
    fn it_reads_what_autark_pkg_prints() {
        let json = r#"{"packages":[{"name":"zlib","version":"1.3.2-1","description":"Compression library","depends":["glibc"],"mode":"staged","imported":false,"installed":null,"held":false,"upgrade":null,"kept":[]}]}"#;
        let all = parse(json).unwrap();
        assert_eq!((all[0].name.as_str(), all[0].state()), ("zlib", State::NotInstalled));
        assert!(parse("not json").unwrap_err().contains("wasn't understood"));
        // A field an older or newer autark-pkg leaves out or adds does not stop the Hub.
        assert_eq!(parse(r#"{"packages":[{"name":"a","version":"1-1","installed":null,"upgrade":null,"extra":1}]}"#).unwrap().len(), 1);
    }

    #[test]
    fn a_package_is_in_one_of_four_states() {
        let all = sample();
        let state = |name: &str| all.iter().find(|package| package.name == name).unwrap().state();
        assert_eq!(state("bash"), State::NotInstalled);
        assert_eq!(state("zlib"), State::Installed);
        assert_eq!(state("curl"), State::Update("8.1-1".into()));
        assert_eq!(state("Old"), State::Held);
    }

    #[test]
    fn rollback_goes_to_a_kept_build_that_is_not_the_installed_one() {
        let all = sample();
        let go = |name: &str| all.iter().find(|package| package.name == name).unwrap().rollback_to();
        assert_eq!(go("Old"), Some("0-9"));
        assert_eq!(go("zlib"), None);
        assert_eq!(go("bash"), None);
    }

    #[test]
    fn views_list_what_they_say_sorted_by_name() {
        let all = sample();
        // Browse hides an Arch import that is not installed, until asked.
        assert_eq!(names(visible(&all, View::Browse, "", false)), ["bash", "curl", "Old", "tree", "zlib"]);
        assert_eq!(names(visible(&all, View::Browse, "", true)), ["bash", "curl", "htop", "Old", "tree", "zlib"]);
        assert_eq!(names(visible(&all, View::Installed, "", false)), ["curl", "Old", "tree", "zlib"]);
        assert_eq!(names(visible(&all, View::Updates, "", false)), ["curl"]);
    }

    #[test]
    fn search_wants_every_word_in_the_name_or_description() {
        let all = sample();
        assert_eq!(names(visible(&all, View::Browse, "COMPRESSION lib", false)), ["zlib"]);
        assert_eq!(names(visible(&all, View::Browse, "data urls", false)), ["curl"]);
        assert!(visible(&all, View::Browse, "compression curl", false).is_empty());
        assert_eq!(names(visible(&all, View::Installed, "z", false)), ["zlib"]);
    }
}
