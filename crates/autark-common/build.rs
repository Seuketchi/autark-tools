//! Embeds every file in `data/looks/` so each tool has the shipped looks without anything being installed. Adding a look
//! is adding a file there: this script finds it.

use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let dir = manifest.join("../../data/looks");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "toml")).collect())
        .unwrap_or_default();
    files.sort();
    let mut code = String::from("/// `(id, file text)` for every file in `data/looks/`, by file name.\npub const BUILTIN: &[(&str, &str)] = &[\n");
    for path in files {
        let id = path.file_stem().and_then(|stem| stem.to_str()).expect("look file names are plain text");
        let absolute = fs::canonicalize(&path).expect("the file was just listed");
        println!("cargo:rerun-if-changed={}", path.display());
        code.push_str(&format!("    ({id:?}, include_str!({:?})),\n", absolute.display().to_string()));
    }
    code.push_str("];\n");
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets this")).join("builtin_looks.rs");
    fs::write(out, code).expect("OUT_DIR is writable");
}
