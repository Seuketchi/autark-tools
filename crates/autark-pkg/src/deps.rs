//! Dependency order: what to build before what, and who still needs a package.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Result, fail};
use crate::recipe::Recipe;

/// `wanted` and every dependency not yet installed, each after what it depends on. A dependency that's
/// installed (at any version) is left alone; upgrades are a separate command.
pub fn install_order(wanted: &[String], recipes: &BTreeMap<String, Recipe>, installed: &BTreeSet<String>) -> Result<Vec<String>> {
    let mut order = Vec::new();
    let mut visiting = Vec::new();
    for name in wanted {
        visit(name, recipes, &|other| installed.contains(other) && !wanted.iter().any(|w| w == other), &mut visiting, &mut order)?;
    }
    Ok(order)
}

/// `names` sorted so each comes after those of them it depends on.
pub fn sort(names: &[String], recipes: &BTreeMap<String, Recipe>) -> Result<Vec<String>> {
    let set: BTreeSet<&String> = names.iter().collect();
    let mut order = Vec::new();
    let mut visiting = Vec::new();
    for name in names {
        visit(name, recipes, &|other| !set.contains(&other.to_owned()), &mut visiting, &mut order)?;
    }
    Ok(order)
}

fn visit(name: &str, recipes: &BTreeMap<String, Recipe>, skip: &dyn Fn(&String) -> bool, visiting: &mut Vec<String>, order: &mut Vec<String>) -> Result<()> {
    if order.iter().any(|done| done == name) {
        return Ok(());
    }
    if let Some(at) = visiting.iter().position(|open| open == name) {
        let mut cycle = visiting[at..].to_vec();
        cycle.push(name.to_owned());
        return fail(format!("dependency cycle: {}", cycle.join(" → ")));
    }
    let Some(recipe) = recipes.get(name) else {
        return match visiting.last() {
            Some(parent) => fail(format!("{parent} depends on {name}, which has no recipe")),
            None => fail(format!("no recipe named {name}")),
        };
    };
    visiting.push(name.to_owned());
    for dependency in &recipe.depends {
        if !skip(dependency) {
            visit(dependency, recipes, skip, visiting, order)?;
        }
    }
    visiting.pop();
    order.push(name.to_owned());
    Ok(())
}

/// Installed packages that depend on `name`, apart from those in `leaving`.
pub fn dependents(name: &str, installed: &BTreeMap<String, Vec<String>>, leaving: &[String]) -> Vec<String> {
    installed.iter().filter(|(other, depends)| !leaving.contains(other) && depends.iter().any(|d| d == name)).map(|(other, _)| other.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Mode;

    fn recipes(edges: &[(&str, &[&str])]) -> BTreeMap<String, Recipe> {
        edges
            .iter()
            .map(|(name, depends)| {
                let recipe = Recipe {
                    name: (*name).into(),
                    version: "1".into(),
                    release: 1,
                    description: String::new(),
                    sources: Vec::new(),
                    depends: depends.iter().map(|d| (*d).to_owned()).collect(),
                    imports: Vec::new(),
                    mode: Mode::Staged,
                };
                ((*name).to_owned(), recipe)
            })
            .collect()
    }

    #[test]
    fn dependencies_come_first_and_installed_ones_are_skipped() {
        let all = recipes(&[("wayfire", &["wlroots", "glib"]), ("wlroots", &["libdrm"]), ("libdrm", &[]), ("glib", &[])]);
        let installed = BTreeSet::from(["glib".to_owned()]);
        assert_eq!(install_order(&["wayfire".into()], &all, &installed).unwrap(), ["libdrm", "wlroots", "wayfire"]);
        assert_eq!(sort(&["wayfire".into(), "glib".into()], &all).unwrap(), ["glib", "wayfire"]);
    }

    #[test]
    fn cycles_and_missing_recipes_are_errors() {
        let all = recipes(&[("a", &["b"]), ("b", &["a"]), ("c", &["nothing"])]);
        assert!(install_order(&["a".into()], &all, &BTreeSet::new()).unwrap_err().0.contains("a → b → a"));
        assert!(install_order(&["c".into()], &all, &BTreeSet::new()).unwrap_err().0.contains("c depends on nothing"));
    }
}
