//! Autark's package manager: signed packages from recipes, with install, remove, upgrade, search and rollback.

mod archive;
mod build;
mod config;
mod db;
mod deps;
mod error;
mod fetch;
mod ops;
mod recipe;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use config::Config;
use db::{Db, Installed, Item, Kind};
use error::{Context, Result, fail};
use recipe::Recipe;

const USAGE: &str = "usage: autark-pkg [options] <command> [names…]

  search <words>       recipes whose name or description has every word
  info <name>          a recipe and what's installed of it
  list                 installed packages
  files <name>         what an installed package owns
  owns <path>          which package owns a file
  install <names>      build (unless a build is cached) and install, dependencies first
  remove <names>       remove, refusing while other packages depend on them
  upgrade [names]      install the recipe's version of packages that differ from it (all by default)
  rollback <name>      go back to the other kept build, and hold it there until installed again
  build <names>        build and sign staged recipes without installing
  import               take over the file lists the phase scripts recorded (/var/lib/autark/files)
  keygen               make the signing key pair
  verify <archive>     check an archive's signature

options:
  --root <dir>         act on a system in <dir> instead of /
  --config <file>      settings (default <root>/etc/autark/pkg.toml)
  --recipes <dir>      recipes folder, overriding the settings
  -n, --dry-run        say what would happen, change nothing
  --force              let a package take over files another one owns";

struct Options {
    dry_run: bool,
    force: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("autark-pkg: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut root, mut config_file, mut recipes_dir) = (PathBuf::from("/"), None, None);
    let mut options = Options { dry_run: false, force: false };
    let mut words = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| error::Error(format!("{flag} needs a value")));
        match arg.as_str() {
            "--root" => root = PathBuf::from(value("--root")?),
            "--config" => config_file = Some(PathBuf::from(value("--config")?)),
            "--recipes" => recipes_dir = Some(PathBuf::from(value("--recipes")?)),
            "-n" | "--dry-run" => options.dry_run = true,
            "--force" => options.force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            flag if flag.starts_with('-') => {
                return fail(format!("unknown option {flag}\n{USAGE}"));
            }
            _ => words.push(arg),
        }
    }
    let Some((command, names)) = words.split_first() else {
        println!("{USAGE}");
        return Ok(());
    };
    let root = std::fs::canonicalize(&root).ctx(&root)?;
    let config = Config::load(&root, config_file.as_deref(), recipes_dir.as_deref())?;
    let changes = matches!(command.as_str(), "install" | "remove" | "upgrade" | "rollback" | "build" | "import" | "keygen");
    if changes && !options.dry_run && config.is_live() && !build::is_root() {
        return fail(format!("{command} changes the system: run it as root"));
    }
    let need = |what: &str| {
        if names.is_empty() { fail(format!("{command} needs {what}")) } else { Ok(()) }
    };
    match command.as_str() {
        "search" => search(&config, names),
        "info" => {
            need("a name")?;
            names.iter().try_for_each(|name| info(&config, name))
        }
        "list" => list(&config),
        "files" => {
            need("a name")?;
            let db = Db::open(&config)?;
            for name in names {
                if db.get(name)?.is_none() {
                    return fail(format!("{name} isn't installed"));
                }
                for item in db.files(name)? {
                    println!("{}{}", item.path, if item.kind == Kind::Dir { "/" } else { "" });
                }
            }
            Ok(())
        }
        "owns" => {
            need("a path")?;
            let owners = Db::open(&config)?.owners()?;
            for path in names {
                match owners.get(path.as_str()) {
                    Some(owner) => println!("{path}: {owner}"),
                    None => println!("{path}: no package"),
                }
            }
            Ok(())
        }
        "install" => {
            need("names")?;
            install(&config, names, &options)
        }
        "remove" => {
            need("names")?;
            remove(&config, names, &options)
        }
        "upgrade" => upgrade(&config, names, &options),
        "rollback" => {
            need("a name")?;
            names.iter().try_for_each(|name| rollback(&config, name, &options))
        }
        "build" => {
            need("names")?;
            let recipes = recipe::load_all(&config.recipes)?;
            for name in names {
                let Some(recipe) = recipes.get(name) else {
                    return fail(format!("no recipe named {name}"));
                };
                if recipe.mode == recipe::Mode::Tracked {
                    return fail(format!("{name} is tracked: it installs as it builds, so use install"));
                }
                if !options.dry_run {
                    let sources = fetch::sources(&config, recipe)?;
                    let built = build::build(&config, recipe, &sources, &Default::default())?;
                    prune(&config, name, None)?;
                    println!("   {}", built.archive.display());
                }
            }
            Ok(())
        }
        "import" => import(&config, &options),
        "keygen" => {
            let path = archive::keygen(&config)?;
            println!("secret key: {} (root only)\npublic key: {}", config.secret_key.display(), path.display());
            println!("check an archive with: minisign -V -p {} -m <archive>", path.display());
            Ok(())
        }
        "verify" => {
            need("an archive")?;
            for path in names {
                let info = archive::verify(&config, Path::new(path))?;
                println!("{path}: good signature, {} {}", info.name, info.version);
            }
            Ok(())
        }
        other => fail(format!("unknown command {other}\n{USAGE}")),
    }
}

fn search(config: &Config, words: &[String]) -> Result<()> {
    let recipes = recipe::load_all(&config.recipes)?;
    let installed = Db::open(config)?.all()?;
    let words: Vec<String> = words.iter().map(|word| word.to_lowercase()).collect();
    for recipe in recipes.values() {
        let haystack = format!("{} {}", recipe.name, recipe.description).to_lowercase();
        if words.iter().all(|word| haystack.contains(word.as_str())) {
            let state = match installed.get(&recipe.name) {
                Some(info) if info.version == recipe.full_version() => "  [installed]".to_owned(),
                Some(info) => format!("  [installed {}]", info.version),
                None => String::new(),
            };
            let description = if recipe.description.is_empty() { String::new() } else { format!("\n    {}", recipe.description) };
            println!("{} {}{state}{description}", recipe.name, recipe.full_version());
        }
    }
    Ok(())
}

fn info(config: &Config, name: &str) -> Result<()> {
    let recipes = recipe::load_all(&config.recipes)?;
    let db = Db::open(config)?;
    let installed = db.get(name)?;
    let recipe = recipes.get(name);
    if recipe.is_none() && installed.is_none() {
        return fail(format!("no recipe or installed package named {name}"));
    }
    println!("{name}");
    if let Some(recipe) = recipe {
        println!("  recipe:      {} ({})", recipe.full_version(), recipe.mode.name());
        if !recipe.description.is_empty() {
            println!("  description: {}", recipe.description);
        }
        if !recipe.depends.is_empty() {
            println!("  depends:     {}", recipe.depends.join(" "));
        }
        for source in &recipe.sources {
            println!("  source:      {} ({})", source.url, source.checksum);
        }
    }
    match installed {
        Some(info) => {
            println!("  installed:   {}{}, {} paths", info.version, if info.held { " (held after a rollback)" } else { "" }, db.files(name)?.len());
            println!("  from:        {}", info.archive.as_deref().unwrap_or("the phase scripts (imported)"));
        }
        None => println!("  installed:   no"),
    }
    let kept = cached(config, name)?;
    if !kept.is_empty() {
        println!("  kept builds: {}", kept.iter().map(|(version, _)| version.as_str()).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

fn list(config: &Config) -> Result<()> {
    for (name, info) in Db::open(config)?.all()? {
        println!("{name} {}{}", info.version, if info.held { " (held)" } else { "" });
    }
    Ok(())
}

fn install(config: &Config, names: &[String], options: &Options) -> Result<()> {
    let recipes = recipe::load_all(&config.recipes)?;
    let db = Db::open(config)?;
    let installed = db.all()?;
    let present: BTreeSet<String> = installed.keys().cloned().collect();
    let order = deps::install_order(names, &recipes, &present)?;
    let order: Vec<String> = order
        .into_iter()
        .filter(|name| {
            let same = installed.get(name).is_some_and(|info| info.version == recipes[name].full_version() && !info.held);
            if same {
                println!("{name} {} is already installed", recipes[name].full_version());
            }
            !same
        })
        .collect();
    if order.is_empty() {
        return Ok(());
    }
    println!("to install: {}", order.iter().map(|name| format!("{name} {}", recipes[name].full_version())).collect::<Vec<_>>().join(", "));
    if options.dry_run {
        return Ok(());
    }
    for name in &order {
        put(config, &db, &recipes[name], options)?;
    }
    Ok(())
}

/// Installs the recipe's version: from the cache if that build is kept, otherwise built first.
fn put(config: &Config, db: &Db, recipe: &Recipe, options: &Options) -> Result<()> {
    let cached = config.cache.join(archive::file_name(&recipe.name, &recipe.full_version()));
    let (archive, extract) = if cached.exists() && archive::signature_path(&cached).exists() {
        println!("== {} {}: from the kept build", recipe.name, recipe.full_version());
        (cached, true)
    } else {
        let sources = fetch::sources(config, recipe)?;
        let built = build::build(config, recipe, &sources, &db.owners()?)?;
        (built.archive, !built.installed)
    };
    ops::install(config, db, &ops::Install { archive: &archive, extract, held: false, force: options.force })?;
    prune(config, &recipe.name, Some(&archive))
}

fn remove(config: &Config, names: &[String], options: &Options) -> Result<()> {
    let db = Db::open(config)?;
    let installed = db.all()?;
    let depends: BTreeMap<String, Vec<String>> = installed.iter().map(|(name, info)| (name.clone(), info.depends.clone())).collect();
    for name in names {
        if !installed.contains_key(name) {
            return fail(format!("{name} isn't installed"));
        }
        let needed_by = deps::dependents(name, &depends, names);
        if !needed_by.is_empty() {
            return fail(format!("{name} is needed by {}", needed_by.join(", ")));
        }
    }
    println!("to remove: {}", names.join(", "));
    if options.dry_run {
        return Ok(());
    }
    names.iter().try_for_each(|name| ops::remove(config, &db, name))
}

fn upgrade(config: &Config, names: &[String], options: &Options) -> Result<()> {
    let recipes = recipe::load_all(&config.recipes)?;
    let db = Db::open(config)?;
    let installed = db.all()?;
    let candidates: Vec<&Installed> = if names.is_empty() {
        installed.values().collect()
    } else {
        names.iter().map(|name| installed.get(name).ok_or_else(|| error::Error(format!("{name} isn't installed")))).collect::<Result<_>>()?
    };
    let (mut due, mut held) = (Vec::new(), 0);
    for info in candidates {
        let Some(recipe) = recipes.get(&info.name) else {
            continue;
        };
        if info.version == recipe.full_version() {
            continue;
        }
        if info.held {
            println!("{} is held at {} after a rollback; `autark-pkg install {}` takes the recipe's {}", info.name, info.version, info.name, recipe.full_version());
            held += 1;
            continue;
        }
        due.push(info.name.clone());
    }
    if due.is_empty() {
        if held == 0 {
            println!("everything matches its recipe");
        }
        return Ok(());
    }
    let due = deps::sort(&due, &recipes)?;
    println!("to upgrade: {}", due.iter().map(|name| format!("{name} {} → {}", installed[name].version, recipes[name].full_version())).collect::<Vec<_>>().join(", "));
    if options.dry_run {
        return Ok(());
    }
    // A new dependency an upgrade brings in is installed first.
    let present: BTreeSet<String> = installed.keys().cloned().collect();
    for name in deps::install_order(&due, &recipes, &present)? {
        put(config, &db, &recipes[&name], options)?;
    }
    Ok(())
}

fn rollback(config: &Config, name: &str, options: &Options) -> Result<()> {
    let db = Db::open(config)?;
    let Some(info) = db.get(name)? else {
        return fail(format!("{name} isn't installed"));
    };
    let Some((version, archive)) = cached(config, name)?.into_iter().find(|(version, _)| *version != info.version) else {
        return fail(format!("no other build of {name} is kept (the cache keeps {})", config.keep));
    };
    println!("to roll back: {name} {} → {version}", info.version);
    if options.dry_run {
        return Ok(());
    }
    ops::install(config, &db, &ops::Install { archive: &archive, extract: true, held: true, force: options.force })
}

/// The kept builds of a package, newest first.
fn cached(config: &Config, name: &str) -> Result<Vec<(String, PathBuf)>> {
    let Ok(entries) = std::fs::read_dir(&config.cache) else {
        return Ok(Vec::new());
    };
    let prefix = format!("{name}-");
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.ctx(&config.cache)?;
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(version) = file.strip_prefix(&prefix).and_then(|rest| rest.strip_suffix(".tar.zst")) else {
            continue;
        };
        // "foo-bar-1.0-1" also starts with "foo-": a version starts with a digit and holds one dash.
        if !version.starts_with(|c: char| c.is_ascii_digit()) || version.matches('-').count() != 1 {
            continue;
        }
        let modified = entry.metadata().and_then(|meta| meta.modified()).ctx(&config.cache)?;
        out.push((modified, version.to_owned(), entry.path()));
    }
    out.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    Ok(out.into_iter().map(|(_, version, path)| (version, path)).collect())
}

/// Keeps the newest `keep` builds of a package, and always the one just installed.
fn prune(config: &Config, name: &str, keep_this: Option<&Path>) -> Result<()> {
    for (_, path) in cached(config, name)?.into_iter().skip(config.keep) {
        if Some(path.as_path()) == keep_this {
            continue;
        }
        std::fs::remove_file(&path).ctx(&path)?;
        let _ = std::fs::remove_file(archive::signature_path(&path));
    }
    Ok(())
}

/// Turns the phase scripts' file lists into installed packages: each list goes to the recipe that names it
/// (by name or in `imports`), at the recipe's version. Lists no recipe claims are steps, not packages.
fn import(config: &Config, options: &Options) -> Result<()> {
    let recipes = recipe::load_all(&config.recipes)?;
    let dir = config.path("/var/lib/autark/files");
    let mut lists: Vec<(std::time::SystemTime, String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir).ctx(&dir)? {
        let entry = entry.ctx(&dir)?;
        let modified = entry.metadata().and_then(|meta| meta.modified()).ctx(&dir)?;
        lists.push((modified, entry.file_name().to_string_lossy().into_owned(), entry.path()));
    }
    // Oldest first, so a rebuild's list (systemd-pam after systemd) wins a path both name.
    lists.sort();
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut claimed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, list, path) in &lists {
        let Some(recipe) = recipes.values().find(|recipe| recipe.name == list.to_lowercase() || recipe.imports.iter().any(|other| other == list)) else {
            println!("skipped {list}: no recipe claims it");
            continue;
        };
        claimed.entry(recipe.name.clone()).or_default().push(list.clone());
        for line in std::fs::read_to_string(path).ctx(path)?.lines().filter(|line| line.starts_with('/')) {
            owner.insert(line.to_owned(), recipe.name.clone());
        }
    }
    let db = Db::open(config)?;
    let mut by_package: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    for (path, name) in owner {
        let real = config.path(&path);
        let Ok(meta) = std::fs::symlink_metadata(&real) else {
            continue;
        };
        let kind = if meta.file_type().is_symlink() {
            Kind::Link
        } else if meta.is_file() {
            Kind::File(Some(fetch::sha256(&real)?))
        } else {
            continue;
        };
        by_package.entry(name).or_default().push(Item { path, kind });
    }
    for (name, items) in by_package {
        let recipe = &recipes[&name];
        if let Some(present) = db.get(&name)? {
            println!("skipped {name}: already installed ({})", present.version);
            continue;
        }
        println!("{name} {}: {} paths from {}", recipe.full_version(), items.len(), claimed[&name].join(", "));
        if options.dry_run {
            continue;
        }
        let info = Installed { name: name.clone(), version: recipe.full_version(), depends: recipe.depends.clone(), mode: recipe.mode.name().into(), installed: db::now(), archive: None, held: false };
        db.save(&info, &items)?;
    }
    Ok(())
}
