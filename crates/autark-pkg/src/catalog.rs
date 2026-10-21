//! `autark-pkg catalog`: every package this system knows, as one JSON document, for programs (the Hub) that show them.
//! A package is known when it has a recipe or is installed. Nothing is changed and nothing is downloaded.
//!
//! ```json
//! {"packages":[{"name":"zlib","version":"1.3.2-1","description":"Compression library","depends":["glibc"],
//!   "mode":"staged","imported":false,"installed":"1.3.1-1","held":false,"upgrade":"1.3.2-1","kept":["1.3.1-1"]}]}
//! ```
//!
//! `version` is the recipe's (the installed one for a package with no recipe); `installed` is null when it isn't
//! installed; `upgrade` is the recipe's version when `autark-pkg upgrade` would install it, null otherwise (a package
//! held after a rollback is never due: `held` says so); `kept` lists the builds in the cache, newest first.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::Config;
use crate::db::{Db, Installed};
use crate::error::Result;
use crate::recipe::Recipe;

#[derive(Debug, Serialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub version: String,
    pub description: String,
    pub depends: Vec<String>,
    pub mode: String,
    pub imported: bool,
    pub installed: Option<String>,
    pub held: bool,
    pub upgrade: Option<String>,
    pub kept: Vec<String>,
}

#[derive(Serialize)]
struct Catalog<'a> {
    packages: &'a [Entry],
}

/// The entries, by name. `kept` maps a name to the versions in the cache.
pub fn entries(recipes: &BTreeMap<String, Recipe>, installed: &BTreeMap<String, Installed>, kept: &BTreeMap<String, Vec<String>>) -> Vec<Entry> {
    let mut names: Vec<&String> = recipes.keys().chain(installed.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let (recipe, have) = (recipes.get(name), installed.get(name));
            let held = have.is_some_and(|have| have.held);
            let upgrade = match (recipe, have) {
                (Some(recipe), Some(have)) if !held && have.version != recipe.full_version() => Some(recipe.full_version()),
                _ => None,
            };
            Entry {
                name: name.clone(),
                version: recipe.map(Recipe::full_version).or_else(|| have.map(|have| have.version.clone())).unwrap_or_default(),
                description: recipe.map(|recipe| recipe.description.clone()).unwrap_or_default(),
                depends: recipe.map(|recipe| recipe.depends.clone()).or_else(|| have.map(|have| have.depends.clone())).unwrap_or_default(),
                mode: recipe.map(|recipe| recipe.mode.name().to_owned()).or_else(|| have.map(|have| have.mode.clone())).unwrap_or_default(),
                imported: recipe.is_some_and(|recipe| recipe.imported),
                installed: have.map(|have| have.version.clone()),
                held,
                upgrade,
                kept: kept.get(name).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

pub fn print(config: &Config) -> Result<()> {
    let recipes = crate::load_recipes(config)?;
    let installed = Db::open_read(config)?.all()?;
    let mut kept = BTreeMap::new();
    for name in recipes.keys().chain(installed.keys()) {
        let builds: Vec<String> = crate::cached(config, name)?.into_iter().map(|(version, _)| version).collect();
        if !builds.is_empty() {
            kept.insert(name.clone(), builds);
        }
    }
    let entries = entries(&recipes, &installed, &kept);
    println!("{}", serde_json::to_string(&Catalog { packages: &entries }).expect("a catalog is plain data"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(name: &str, version: &str, imported: bool) -> Recipe {
        let text = format!("name={name}\nversion={version}\nrelease=1\ndescription=\"The {name}\"\nsource=()\nchecksums=()\ndepends=(glibc)\nmode=staged\n\nbuild() {{\n:\n}}\n");
        let mut recipe = Recipe::parse(&text, &format!("{name}.sh")).unwrap();
        recipe.imported = imported;
        recipe
    }

    fn have(name: &str, version: &str, held: bool) -> Installed {
        Installed { name: name.into(), version: version.into(), depends: vec!["glibc".into()], mode: "tracked".into(), installed: 0, archive: None, held }
    }

    #[test]
    fn it_joins_recipes_and_what_is_installed_the_way_upgrade_does() {
        let recipes: BTreeMap<_, _> = [("curl", recipe("curl", "8.1", false)), ("old", recipe("old", "2", false)), ("new", recipe("new", "1", true)), ("same", recipe("same", "3", false))]
            .into_iter()
            .map(|(name, recipe)| (name.to_owned(), recipe))
            .collect();
        let installed: BTreeMap<_, _> = [("curl", have("curl", "8.0-1", false)), ("old", have("old", "1-1", true)), ("same", have("same", "3-1", false)), ("orphan", have("orphan", "9-2", false))]
            .into_iter()
            .map(|(name, info)| (name.to_owned(), info))
            .collect();
        let kept = BTreeMap::from([("curl".to_owned(), vec!["8.0-1".to_owned(), "7.9-1".to_owned()])]);
        let all = entries(&recipes, &installed, &kept);
        let by = |name: &str| all.iter().find(|entry| entry.name == name).unwrap();

        assert_eq!(all.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["curl", "new", "old", "orphan", "same"]);
        let curl = by("curl");
        assert_eq!((curl.version.as_str(), curl.installed.as_deref(), curl.upgrade.as_deref()), ("8.1-1", Some("8.0-1"), Some("8.1-1")));
        assert_eq!((curl.description.as_str(), curl.depends.as_slice(), curl.mode.as_str(), curl.kept.as_slice()), ("The curl", ["glibc".to_owned()].as_slice(), "staged", ["8.0-1".to_owned(), "7.9-1".to_owned()].as_slice()));
        // Held after a rollback: never due, as in `upgrade`.
        assert!(by("old").held && by("old").upgrade.is_none());
        // Not installed, and imported from Arch.
        let new = by("new");
        assert!(new.imported && new.installed.is_none() && new.upgrade.is_none());
        assert!(by("same").upgrade.is_none());
        // Installed with no recipe: what the database says.
        let orphan = by("orphan");
        assert_eq!((orphan.version.as_str(), orphan.mode.as_str(), orphan.installed.as_deref(), orphan.description.as_str()), ("9-2", "tracked", Some("9-2"), ""));
    }

    #[test]
    fn the_json_has_the_shape_programs_read() {
        let entry = Entry { name: "zlib".into(), version: "1.3.2-1".into(), description: "Compression library".into(), depends: vec!["glibc".into()], mode: "staged".into(), imported: false, installed: None, held: false, upgrade: None, kept: vec![] };
        let json = serde_json::to_string(&Catalog { packages: &[entry] }).unwrap();
        assert_eq!(json, r#"{"packages":[{"name":"zlib","version":"1.3.2-1","description":"Compression library","depends":["glibc"],"mode":"staged","imported":false,"installed":null,"held":false,"upgrade":null,"kept":[]}]}"#);
    }
}
