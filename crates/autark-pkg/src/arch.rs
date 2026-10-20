//! Recipes imported from Arch Linux. `scripts/arch-import.py` reads a package's PKGBUILD, downloads and checks its
//! sources and writes an autark recipe into `config.imported`; `recipe::load_imported` reads them from there, below
//! the hand-written ones. The importer runs as the build user, so evaluating a PKGBUILD and fetching from the
//! network never happens as root.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::build;
use crate::config::Config;
use crate::error::{Context, Result, fail};
use crate::recipe::Recipe;

/// Imports `names` and the runtime dependencies they need that no recipe has, into `config.imported`.
pub fn import(config: &Config, names: &[String]) -> Result<()> {
    let Some(imported) = &config.imported else {
        return fail("recipes come from --recipes here, so nothing can be imported");
    };
    if !config.arch_importer.exists() {
        return fail(format!("no importer at {}: it is scripts/arch-import.py in the autark repo", config.arch_importer.display()));
    }
    std::fs::create_dir_all(imported).ctx(imported)?;
    std::fs::create_dir_all(&config.arch_cache).ctx(&config.arch_cache)?;
    let mut command = Command::new("python3");
    command
        .arg("-I")
        .arg(&config.arch_importer)
        .args(["-r", "--out"])
        .arg(imported)
        .arg("--recipes")
        .arg(&config.recipes)
        .arg("--cache")
        .arg(&config.arch_cache)
        .args(names)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &config.arch_cache)
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null());
    if config.is_live() && build::is_root() {
        let Some((uid, gid)) = build::user(&config.build_user) else {
            return fail(format!("no user {}: the importer does not run as root", config.build_user));
        };
        for dir in [imported, &config.arch_cache] {
            std::os::unix::fs::lchown(dir, Some(uid), Some(gid)).ctx(dir)?;
        }
        // SAFETY: drop_to makes only async-signal-safe calls, as is required between fork and exec.
        unsafe { command.pre_exec(move || build::drop_to(uid, gid)) };
    }
    println!("== importing {} from Arch Linux", names.join(", "));
    let status = command.status().ctx("python3")?;
    if !status.success() {
        return fail(format!("the importer failed for {}", names.join(", ")));
    }
    Ok(())
}

/// Whether to import: `-y` says yes; otherwise a person at a terminal is asked, and a script has to pass `-y`.
pub fn confirm(missing: &[String], yes: bool) -> Result<()> {
    println!("not in Autark's recipes: {}", missing.join(", "));
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return fail("import them from Arch Linux with `autark-pkg arch <names>`, or pass -y");
    }
    print!("import {} from Arch Linux, and build it as the build user? [y/N] ", if missing.len() == 1 { "it" } else { "them" });
    std::io::stdout().flush().ctx("stdout")?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ctx("stdin")?;
    if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") { Ok(()) } else { fail("not imported") }
}

/// The imported recipes' names, for `refresh` with no names.
pub fn imported_names(recipes: &BTreeMap<String, Recipe>) -> Vec<String> {
    recipes.values().filter(|recipe| recipe.imported).map(|recipe| recipe.name.clone()).collect()
}
