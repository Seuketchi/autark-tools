//! Sources: downloaded once into the sources folder, and checked against the recipe every time they're used.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use md5::Digest;

use crate::config::Config;
use crate::error::{Context, Result, fail};
use crate::recipe::{Recipe, Source};

/// The recipe's sources, each present and matching its checksum.
pub fn sources(config: &Config, recipe: &Recipe) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(&config.sources).ctx(&config.sources)?;
    recipe.sources.iter().map(|source| one(config, recipe, source)).collect()
}

fn one(config: &Config, recipe: &Recipe, source: &Source) -> Result<PathBuf> {
    if source.checksum == "-" {
        return fail(format!("{}: no checksum recorded for {}; add one to {}.sh before building", recipe.name, source.file, recipe.name));
    }
    let path = config.sources.join(&source.file);
    if !path.exists() {
        println!("   downloading {}", source.url);
        download(&source.url, &path)?;
    }
    let actual = checksum(&path, &source.checksum)?;
    if actual != source.checksum {
        return fail(format!("{}: {} doesn't match its checksum\n  recipe: {}\n  file:   {actual}\nDelete it to download again.", recipe.name, path.display(), source.checksum));
    }
    Ok(path)
}

fn download(url: &str, path: &Path) -> Result<()> {
    let partial = path.with_extension("part");
    let response = ureq::get(url).call().ctx(url)?;
    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(&partial).ctx(&partial)?;
    std::io::copy(&mut reader, &mut file).ctx(url)?;
    file.flush().ctx(&partial)?;
    std::fs::rename(&partial, path).ctx(path)
}

/// The file's checksum in the same kind and form as `like` (`md5:…` or `sha256:…`).
pub fn checksum(path: &Path, like: &str) -> Result<String> {
    let mut file = std::fs::File::open(path).ctx(path)?;
    if like.starts_with("md5:") { Ok(format!("md5:{}", hex(&digest::<md5::Md5>(&mut file).ctx(path)?))) } else { Ok(format!("sha256:{}", hex(&digest::<sha2::Sha256>(&mut file).ctx(path)?))) }
}

pub fn digest<D: Digest>(reader: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut hasher = D::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finalize().to_vec());
        }
        hasher.update(&buffer[..read]);
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A file's sha256, as the file lists record it.
pub fn sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).ctx(path)?;
    Ok(hex(&digest::<sha2::Sha256>(&mut file).ctx(path)?))
}
