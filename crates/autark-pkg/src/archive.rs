//! Package archives: `<name>-<version>-<release>.tar.zst`, starting with a `.PKGINFO`, each with a minisign
//! signature beside it (`.minisig`) that the standard `minisign -V` also checks. Nothing installs from an
//! archive whose signature doesn't verify against a key in the keys folder.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Context, Error, Result, fail};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PkgInfo {
    pub name: String,
    /// `version-release`.
    pub version: String,
    #[serde(default)]
    pub depends: Vec<String>,
    pub mode: String,
    /// Seconds since the epoch.
    pub built: u64,
}

impl PkgInfo {
    pub fn file_name(&self) -> String {
        file_name(&self.name, &self.version)
    }

    /// What the signature's trusted comment must say, so a validly signed archive can't pass as another.
    fn trusted_comment(&self) -> String {
        format!("autark-pkg {} {}", self.name, self.version)
    }
}

pub fn file_name(name: &str, version: &str) -> String {
    format!("{name}-{version}.tar.zst")
}

pub fn signature_path(archive: &Path) -> PathBuf {
    let mut name = archive.as_os_str().to_owned();
    name.push(".minisig");
    PathBuf::from(name)
}

/// Packs `paths` (absolute on the target system, found under `from`) into `out`, then signs it.
/// `root_owned` records everything as root's, as a staged build's files are the build user's.
pub fn pack(config: &Config, from: &Path, paths: &[String], info: &PkgInfo, root_owned: bool, out: &Path) -> Result<()> {
    let temporary = out.with_extension("part");
    let file = File::create(&temporary).ctx(&temporary)?;
    let mut encoder = zstd::Encoder::new(file, 9).ctx("zstd")?;
    encoder.multithread(std::thread::available_parallelism().map_or(1, |n| n.get() as u32)).ctx("zstd")?;
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);

    let pkginfo = toml::to_string(info).ctx("PKGINFO")?;
    let mut header = tar::Header::new_gnu();
    header.set_size(pkginfo.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(info.built);
    header.set_entry_type(tar::EntryType::Regular);
    builder.append_data(&mut header, ".PKGINFO", pkginfo.as_bytes()).ctx(&temporary)?;

    for path in paths {
        if path.contains('\n') {
            return fail(format!("{path:?}: a newline in a file name can't go in a file list"));
        }
        let source = crate::config::under(from, Path::new(path));
        let name = path.trim_start_matches('/');
        let meta = std::fs::symlink_metadata(&source).ctx(&source)?;
        let mut header = tar::Header::new_gnu();
        header.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
        if root_owned {
            header.set_uid(0);
            header.set_gid(0);
        }
        let kind = meta.file_type();
        if kind.is_symlink() {
            let target = std::fs::read_link(&source).ctx(&source)?;
            builder.append_link(&mut header, name, &target).ctx(&source)?;
        } else if kind.is_dir() {
            builder.append_data(&mut header, name, std::io::empty()).ctx(&source)?;
        } else if kind.is_file() {
            let file = File::open(&source).ctx(&source)?;
            builder.append_data(&mut header, name, BufReader::new(file)).ctx(&source)?;
        } else {
            println!("   skipped {path}: not a file, folder or symlink");
        }
    }
    let encoder = builder.into_inner().ctx(&temporary)?;
    encoder.finish().ctx(&temporary)?.sync_all().ctx(&temporary)?;
    std::fs::rename(&temporary, out).ctx(out)?;
    sign(config, out, info)
}

fn sign(config: &Config, archive: &Path, info: &PkgInfo) -> Result<()> {
    if !config.secret_key.exists() {
        return fail(format!("no signing key at {}; create one with `autark-pkg keygen`", config.secret_key.display()));
    }
    let secret = minisign::SecretKey::from_file(&config.secret_key, Some(String::new())).ctx(&config.secret_key)?;
    let public = minisign::PublicKey::from_secret_key(&secret).ctx("public key")?;
    let file = File::open(archive).ctx(archive)?;
    let signature = minisign::sign(Some(&public), &secret, BufReader::new(file), Some(&info.trusted_comment()), Some("signed by autark-pkg")).ctx("signing")?;
    let path = signature_path(archive);
    std::fs::write(&path, signature.into_string()).ctx(&path)
}

/// The archive's `.PKGINFO`, once its signature has verified against one of the trusted keys.
pub fn verify(config: &Config, archive: &Path) -> Result<PkgInfo> {
    let signature_file = signature_path(archive);
    let signature = minisign::SignatureBox::from_file(&signature_file).ctx(&signature_file)?;
    let keys = trusted_keys(config)?;
    if keys.is_empty() {
        return fail(format!("no trusted keys in {}; create one with `autark-pkg keygen`", config.keys.display()));
    }
    let mut last = Error(format!("{}: signed by a key not in {}", archive.display(), config.keys.display()));
    let mut verified = false;
    for key in &keys {
        let file = File::open(archive).ctx(archive)?;
        match minisign::verify(key, &signature, BufReader::new(file), true, false, false) {
            Ok(()) => {
                verified = true;
                break;
            }
            Err(error) if !error.to_string().contains("different from public key") => last = Error(format!("{}: {error}", archive.display())),
            Err(_) => {}
        }
    }
    if !verified {
        return Err(last);
    }
    let info = read_info(archive)?;
    if signature.trusted_comment().ctx(&signature_file)? != info.trusted_comment() {
        return fail(format!("{}: the signature is for another package", archive.display()));
    }
    Ok(info)
}

fn trusted_keys(config: &Config) -> Result<Vec<minisign::PublicKey>> {
    let Ok(entries) = std::fs::read_dir(&config.keys) else {
        return Ok(Vec::new());
    };
    let mut keys = Vec::new();
    for entry in entries {
        let path = entry.ctx(&config.keys)?.path();
        if path.extension().is_some_and(|extension| extension == "pub") {
            keys.push(minisign::PublicKey::from_file(&path).ctx(&path)?);
        }
    }
    Ok(keys)
}

fn open(archive: &Path) -> Result<tar::Archive<zstd::Decoder<'static, BufReader<File>>>> {
    let file = File::open(archive).ctx(archive)?;
    Ok(tar::Archive::new(zstd::Decoder::new(file).ctx(archive)?))
}

/// Reads `.PKGINFO` without checking the signature: call `verify` before trusting it.
fn read_info(archive: &Path) -> Result<PkgInfo> {
    let mut tar = open(archive)?;
    let mut entries = tar.entries().ctx(archive)?;
    let Some(entry) = entries.next() else {
        return fail(format!("{}: empty archive", archive.display()));
    };
    let mut entry = entry.ctx(archive)?;
    if entry.path().ctx(archive)?.as_os_str() != ".PKGINFO" {
        return fail(format!("{}: doesn't start with .PKGINFO", archive.display()));
    }
    let mut text = String::new();
    entry.read_to_string(&mut text).ctx(archive)?;
    toml::from_str(&text).ctx(archive)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Link(PathBuf),
    Dir,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Absolute on the target system.
    pub path: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
}

/// Walks a verified archive, handing each entry and (for files) its contents to `each`.
pub fn walk(archive: &Path, mut each: impl FnMut(&Entry, &mut dyn Read) -> Result<()>) -> Result<()> {
    let mut tar = open(archive)?;
    for entry in tar.entries().ctx(archive)?.skip(1) {
        let mut entry = entry.ctx(archive)?;
        let raw = entry.path().ctx(archive)?.into_owned();
        if raw.is_absolute() || raw.components().any(|part| matches!(part, Component::ParentDir)) {
            return fail(format!("{}: unsafe path {}", archive.display(), raw.display()));
        }
        let header = entry.header();
        let kind = match header.entry_type() {
            tar::EntryType::Regular => EntryKind::File,
            tar::EntryType::Directory => EntryKind::Dir,
            tar::EntryType::Symlink => EntryKind::Link(entry.link_name().ctx(archive)?.map(|target| target.into_owned()).unwrap_or_default()),
            other => {
                return fail(format!("{}: {} is a {other:?}, which packages can't hold", archive.display(), raw.display()));
            }
        };
        let item = Entry {
            path: format!("/{}", raw.to_string_lossy().trim_end_matches('/')),
            kind,
            mode: header.mode().ctx(archive)?,
            uid: header.uid().ctx(archive)? as u32,
            gid: header.gid().ctx(archive)? as u32,
            mtime: header.mtime().ctx(archive)?,
        };
        each(&item, &mut entry)?;
    }
    Ok(())
}

/// Every path under `dir`, as absolute paths on the target system, parents before children.
pub fn tree(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let mut children: Vec<PathBuf> = std::fs::read_dir(&next).ctx(&next)?.map(|entry| entry.map(|entry| entry.path())).collect::<std::io::Result<_>>().ctx(&next)?;
        children.sort();
        for child in children.into_iter().rev() {
            if std::fs::symlink_metadata(&child).ctx(&child)?.is_dir() {
                stack.push(child.clone());
            }
            out.push(format!("/{}", child.strip_prefix(dir).unwrap_or(&child).to_string_lossy()));
        }
    }
    out.sort();
    Ok(out)
}

/// Makes a key pair: the secret key root-only, the public key into the keys folder.
pub fn keygen(config: &Config) -> Result<PathBuf> {
    if config.secret_key.exists() {
        return fail(format!("{} already exists; move it away first to make a new one", config.secret_key.display()));
    }
    // An empty password leaves the key unencrypted but checksummed; the crate's own "unencrypted" pair has no
    // checksum and can't be loaded again. A zero KDF id then marks it unencrypted, as `minisign -W` writes it.
    let pair = minisign::KeyPair::generate_encrypted_keypair(Some(String::new())).ctx("key generation")?;
    let mut bytes = pair.sk.to_bytes();
    bytes[2..4].fill(0);
    let unencrypted = minisign::SecretKey::from_bytes(&bytes).ctx("secret key")?;
    let id = pair.pk.keynum().iter().rev().map(|byte| format!("{byte:02X}")).collect::<String>();
    let public = pair.pk.to_box().ctx("public key")?;
    let secret = unencrypted.to_box(Some("autark-pkg signing key")).ctx("secret key")?;
    if let Some(parent) = config.secret_key.parent() {
        std::fs::create_dir_all(parent).ctx(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode_0600().open(&config.secret_key).ctx(&config.secret_key)?;
    file.write_all(secret.into_string().as_bytes()).ctx(&config.secret_key)?;
    std::fs::create_dir_all(&config.keys).ctx(&config.keys)?;
    let path = config.keys.join(format!("{id}.pub"));
    std::fs::write(&path, public.into_string()).ctx(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).ctx(&path)?;
    Ok(path)
}

trait Private {
    fn mode_0600(&mut self) -> &mut Self;
}

impl Private for std::fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::mode(self, 0o600)
    }
}
