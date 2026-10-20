//! Recipes: `<recipes>/<name>.sh`, a bash file whose header is plain assignments autark-pkg reads without
//! running anything, followed by a `build()` function the build runs.
//!
//! ```sh
//! name=zlib
//! version=1.3.2
//! release=1
//! description="Compression library"
//! source=(https://zlib.net/fossils/zlib-$version.tar.gz)    # file::url saves under another name
//! checksums=(md5:a1e6c958597af3c67d162995a342138a)            # md5: or sha256:, one per source
//! depends=(glibc)
//! imports=(zlib-old)        # file lists the phase scripts recorded under other names
//! mode=staged               # staged: installs into $DESTDIR; tracked: installs into / as root
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Context, Result, fail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Built unprivileged into a staging folder, packed and signed, then installed from the archive.
    Staged,
    /// Built and installed as root in place; its new and changed files are found afterwards and packed.
    Tracked,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Staged => "staged",
            Mode::Tracked => "tracked",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "staged" => Some(Mode::Staged),
            "tracked" => Some(Mode::Tracked),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The name it's saved and unpacked under.
    pub file: String,
    pub url: String,
    /// `md5:<hex>` or `sha256:<hex>`; `-` when nobody has recorded one yet.
    pub checksum: String,
}

#[derive(Clone, Debug)]
pub struct Recipe {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub description: String,
    pub sources: Vec<Source>,
    pub depends: Vec<String>,
    pub imports: Vec<String>,
    pub mode: Mode,
    /// The file it was read from, which the build sources.
    pub file: Option<PathBuf>,
    /// Whether it came from Arch Linux (`autark-pkg arch`), not from the autark repo.
    pub imported: bool,
}

impl Recipe {
    /// `version-release`, as packages are named and compared.
    pub fn full_version(&self) -> String {
        format!("{}-{}", self.version, self.release)
    }

    pub fn parse(text: &str, file_name: &str) -> Result<Self> {
        let header = header(text).ctx(file_name)?;
        let single = |key: &str| header.get(key).and_then(|values| values.first().cloned());
        let name = single("name").ok_or_else(|| crate::error::Error(format!("{file_name}: no name=")))?;
        if file_name.strip_suffix(".sh") != Some(name.as_str()) {
            return fail(format!("{file_name}: name={name} doesn't match the file name"));
        }
        let Some(version) = single("version") else {
            return fail(format!("{file_name}: no version="));
        };
        let release = single("release").map_or(Ok(1), |text| text.parse()).ctx(format!("{file_name}: release"))?;
        let expand = |text: &str| text.replace("${version}", &version).replace("$version", &version).replace("${name}", &name).replace("$name", &name);
        let list = |key: &str| header.get(key).cloned().unwrap_or_default().iter().map(|value| expand(value)).collect::<Vec<_>>();
        let (urls, checksums) = (list("source"), list("checksums"));
        if urls.len() != checksums.len() {
            return fail(format!("{file_name}: {} sources but {} checksums", urls.len(), checksums.len()));
        }
        let mut sources = Vec::new();
        for (entry, checksum) in urls.into_iter().zip(checksums) {
            let (file, url) = match entry.split_once("::") {
                Some((file, url)) => (file.to_owned(), url.to_owned()),
                None => (entry.rsplit('/').next().unwrap_or(&entry).to_owned(), entry.clone()),
            };
            if file.is_empty() || file.contains('/') || file.starts_with('.') {
                return fail(format!("{file_name}: bad source file name {file:?}"));
            }
            if checksum != "-" && !valid_checksum(&checksum) {
                return fail(format!("{file_name}: checksum {checksum:?} isn't md5:<32 hex> or sha256:<64 hex>"));
            }
            sources.push(Source { file, url, checksum });
        }
        let mode = single("mode").unwrap_or_else(|| "staged".into());
        let Some(mode) = Mode::parse(&mode) else {
            return fail(format!("{file_name}: mode={mode} isn't staged or tracked"));
        };
        if !text.lines().any(|line| line.trim_start().starts_with("build()")) {
            return fail(format!("{file_name}: no build() function"));
        }
        let description = single("description").map(|text| expand(&text)).unwrap_or_default();
        let (depends, imports) = (list("depends"), list("imports"));
        Ok(Self { name: name.clone(), version: version.clone(), release, description, sources, depends, imports, mode, file: None, imported: false })
    }
}

fn valid_checksum(text: &str) -> bool {
    let hex = |value: &str, length: usize| value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    match text.split_once(':') {
        Some(("md5", value)) => hex(value, 32),
        Some(("sha256", value)) => hex(value, 64),
        _ => false,
    }
}

/// The assignments before the first function, as lists (a plain value is a list of one).
fn header(text: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let mut out = BTreeMap::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // The first function ends the header. An empty list (`depends=()`) is an assignment, not a function.
        let assignment = line.split_once('=').is_some_and(|(key, _)| !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
        if line.contains("()") && !assignment {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            return fail(format!("not an assignment: {line}"));
        };
        if !key.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_') {
            return fail(format!("not an assignment: {line}"));
        }
        let values = if let Some(rest) = value.strip_prefix('(') {
            let mut joined = rest.to_owned();
            while !joined.trim_end().ends_with(')') {
                let Some(next) = lines.next() else {
                    return fail(format!("{key}=( never closes"));
                };
                joined.push(' ');
                joined.push_str(next.trim());
            }
            let inner = joined.trim_end().strip_suffix(')').unwrap_or(&joined);
            words(inner)
        } else {
            words(value).into_iter().take(1).collect()
        };
        out.insert(key.to_owned(), values);
    }
    Ok(out)
}

/// Splits on spaces, honouring double and single quotes, and stops at a comment.
fn words(text: &str) -> Vec<String> {
    let (mut out, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for c in text.chars() {
        match (quote, c) {
            (None, '#') if !started => break,
            (None, ' ' | '\t') => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (Some(q), c) if c == q => quote = None,
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(word);
    }
    out
}

/// Every recipe in the folder, by name. A recipe that doesn't parse is an error: a broken one would
/// otherwise look like a missing package.
pub fn load_all(dir: &Path) -> Result<BTreeMap<String, Recipe>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).ctx(dir)? {
        let path = entry.ctx(dir)?.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.ends_with(".sh") || file_name == "lib.sh" {
            continue;
        }
        let mut recipe = Recipe::parse(&std::fs::read_to_string(&path).ctx(&path)?, file_name)?;
        recipe.file = Some(path);
        out.insert(recipe.name.clone(), recipe);
    }
    Ok(out)
}

/// The recipes `autark-pkg arch` imported. That folder is written by the build user, so what is read from it is held
/// to less: a recipe that doesn't parse is left out with a warning, not an error that stops everything, and one that
/// isn't `staged` (which would run as root) is refused.
pub fn load_imported(dir: &Path) -> BTreeMap<String, Recipe> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.ends_with(".sh") {
            continue;
        }
        let parsed = std::fs::read_to_string(&path).map_err(|error| crate::error::Error(error.to_string())).and_then(|text| Recipe::parse(&text, file_name));
        match parsed {
            Ok(mut recipe) if recipe.mode == Mode::Staged => {
                recipe.file = Some(path);
                recipe.imported = true;
                recipe.imports.clear();
                out.insert(recipe.name.clone(), recipe);
            }
            Ok(recipe) => eprintln!("autark-pkg: skipped imported {}: it is {}, and only staged recipes are taken from there", recipe.name, recipe.mode.name()),
            Err(error) => eprintln!("autark-pkg: skipped imported recipe: {error}"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZLIB: &str = "# zlib\nname=zlib\nversion=1.3.2\nrelease=2\ndescription=\"A library\"  # what it is\n\
        source=(https://zlib.net/zlib-$version.tar.gz\n  extra.patch::https://example.org/p)\nchecksums=(md5:a1e6c958597af3c67d162995a342138a -)\n\
        depends=(glibc)\nmode=tracked\n\nbuild() {\nmake\n}\n";

    #[test]
    fn parses_the_header() {
        let recipe = Recipe::parse(ZLIB, "zlib.sh").unwrap();
        assert_eq!(recipe.full_version(), "1.3.2-2");
        assert_eq!(recipe.description, "A library");
        assert_eq!(recipe.sources[0].file, "zlib-1.3.2.tar.gz");
        assert_eq!(recipe.sources[0].url, "https://zlib.net/zlib-1.3.2.tar.gz");
        assert_eq!(recipe.sources[1], Source { file: "extra.patch".into(), url: "https://example.org/p".into(), checksum: "-".into() });
        assert_eq!(recipe.depends, ["glibc"]);
        assert_eq!(recipe.mode, Mode::Tracked);
    }

    #[test]
    fn an_empty_list_does_not_end_the_header() {
        let text = "name=zlib\nversion=1\nsource=()\nchecksums=()\ndepends=()\nmode=tracked\n\nbuild() {\n:\n}\n";
        assert_eq!(Recipe::parse(text, "zlib.sh").unwrap().mode, Mode::Tracked);
    }

    #[test]
    fn refuses_broken_recipes() {
        assert!(Recipe::parse(ZLIB, "other.sh").is_err());
        assert!(Recipe::parse(&ZLIB.replace(" -)", ")"), "zlib.sh").is_err());
        assert!(Recipe::parse(&ZLIB.replace("md5:a1e6", "md5:zz"), "zlib.sh").is_err());
        assert!(Recipe::parse(&ZLIB.replace("build()", "make()"), "zlib.sh").is_err());
        assert!(Recipe::parse(&ZLIB.replace("mode=tracked", "mode=fast"), "zlib.sh").is_err());
    }
}

#[cfg(test)]
mod imported_tests {
    use super::*;

    fn recipe_text(name: &str, mode: &str) -> String {
        format!("name={name}\nversion=1\nsource=()\nchecksums=()\ndepends=()\nmode={mode}\n\nbuild() {{\n:\n}}\n")
    }

    #[test]
    fn imported_recipes_are_staged_only_and_a_broken_one_does_not_stop_the_rest() {
        let dir = std::env::temp_dir().join(format!("autark-pkg-imported-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("good.sh"), recipe_text("good", "staged")).unwrap();
        std::fs::write(dir.join("rooty.sh"), recipe_text("rooty", "tracked")).unwrap();
        std::fs::write(dir.join("broken.sh"), "this is not a recipe\n").unwrap();
        std::fs::write(dir.join("mismatch.sh"), recipe_text("other", "staged")).unwrap();
        let loaded = load_imported(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(loaded.keys().collect::<Vec<_>>(), ["good"]);
        assert!(loaded["good"].imported);
        assert!(loaded["good"].file.as_ref().is_some_and(|file| file.ends_with("good.sh")));
    }
}
