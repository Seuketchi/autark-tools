//! Prebuilt packages from a repository, and publishing them. A repository is a folder served over HTTP(S) holding
//! `<name>-<version>-<release>.tar.zst` archives, each with its `.minisig`, and an `index` (with `index.minisig`)
//! that lists them for people. Releases are signed with the release key, which stays offline: `publish` re-signs the
//! archives built here with it. A system trusts the release key by having its `.pub` in the keys folder, and checks
//! every download against its keys exactly as it checks an archive it built.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::archive::{self, PkgInfo};
use crate::config::Config;
use crate::error::{Context, Error, Result, fail};
use crate::fetch;

/// The repository's archive for `name` `version` (`version-release`), downloaded into the cache after its signature
/// and package info check out; `None` when the repository doesn't have it (or none is set), so the caller builds.
pub fn prebuilt(config: &Config, name: &str, version: &str) -> Result<Option<PathBuf>> {
    let Some(base) = &config.repo else {
        return Ok(None);
    };
    let file = archive::file_name(name, version);
    let staging = config.cache.join(".fetch");
    std::fs::create_dir_all(&staging).ctx(&staging)?;
    let archive_path = staging.join(&file);
    let signature_path = archive::signature_path(&archive_path);
    let clean = || {
        let _ = std::fs::remove_file(&archive_path);
        let _ = std::fs::remove_file(&signature_path);
    };
    println!("   looking for {file} in {base}");
    for (url, path) in [(format!("{base}/{file}"), &archive_path), (format!("{base}/{file}.minisig"), &signature_path)] {
        match download(&url, path) {
            Ok(true) => {}
            Ok(false) => {
                clean();
                return Ok(None);
            }
            Err(error) => {
                clean();
                println!("   the repository can't be reached ({error}); building instead");
                return Ok(None);
            }
        }
    }
    // Not trusted until the signature verifies, and the archive says it is this package: a validly signed older
    // or different archive served under this name is refused.
    let info = match archive::verify(config, &archive_path) {
        Ok(info) => info,
        Err(error) => {
            clean();
            return fail(format!("{file} from the repository was refused: {error}"));
        }
    };
    if info.name != name || info.version != version {
        clean();
        return fail(format!("{file} from the repository is {} {}, not {name} {version}", info.name, info.version));
    }
    std::fs::create_dir_all(&config.cache).ctx(&config.cache)?;
    let kept = config.cache.join(&file);
    std::fs::rename(&archive_path, &kept).ctx(&kept)?;
    std::fs::rename(&signature_path, archive::signature_path(&kept)).ctx(&kept)?;
    Ok(Some(kept))
}

/// Downloads `url` to `path`; false when the server says there is no such file.
fn download(url: &str, path: &Path) -> Result<bool> {
    let response = match ureq::get(url).call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(false),
        Err(error) => return fail(format!("{url}: {error}")),
    };
    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(path).ctx(path)?;
    std::io::copy(&mut reader, &mut file).ctx(url)?;
    file.flush().ctx(path)?;
    Ok(true)
}

/// Copies the newest cached build of each package (or of `names`) into `out`, signs the copies with the release
/// key, and writes `index` and `index.minisig`. The cached archives must verify against the keys here first.
pub fn publish(config: &Config, out: &Path, key: &Path, password: &str, names: &[String]) -> Result<usize> {
    let mut newest: BTreeMap<String, (PkgInfo, PathBuf)> = BTreeMap::new();
    let entries = std::fs::read_dir(&config.cache).ctx(&config.cache)?;
    for entry in entries {
        let path = entry.ctx(&config.cache)?.path();
        if !path.to_string_lossy().ends_with(".tar.zst") || !archive::signature_path(&path).exists() {
            continue;
        }
        let info = archive::verify(config, &path)?;
        if !names.is_empty() && !names.contains(&info.name) {
            continue;
        }
        if newest.get(&info.name).is_none_or(|(old, _)| old.built < info.built) {
            newest.insert(info.name.clone(), (info, path));
        }
    }
    for name in names {
        if !newest.contains_key(name) {
            return fail(format!("no cached build of {name} in {}", config.cache.display()));
        }
    }
    if newest.is_empty() {
        return fail(format!("no signed archives in {}", config.cache.display()));
    }
    std::fs::create_dir_all(out).ctx(out)?;
    let mut index = String::new();
    for (name, (info, path)) in &newest {
        let target = out.join(info.file_name());
        std::fs::copy(path, &target).ctx(&target)?;
        archive::sign_with(&target, key, password.to_owned(), info)?;
        let size = std::fs::metadata(&target).ctx(&target)?.len();
        index.push_str(&format!("{name} {} {} {size} {}\n", info.version, fetch::sha256(&target)?, info.file_name()));
    }
    let index_path = out.join("index");
    std::fs::write(&index_path, &index).ctx(&index_path)?;
    let secret = minisign::SecretKey::from_file(key, Some(password.to_owned())).ctx(format!("{} (wrong password?)", key.display()))?;
    let public = minisign::PublicKey::from_secret_key(&secret).ctx("public key")?;
    let file = std::fs::File::open(&index_path).ctx(&index_path)?;
    let signature = minisign::sign(Some(&public), &secret, std::io::BufReader::new(file), Some("autark-pkg index"), Some("signed by autark-pkg")).ctx("signing the index")?;
    let signature_path = archive::signature_path(&index_path);
    std::fs::write(&signature_path, signature.into_string()).ctx(&signature_path)?;
    Ok(newest.len())
}

/// A password typed without echo (the terminal is restored even on an error), or read from `file`.
pub fn password(prompt: &str, file: Option<&Path>) -> Result<String> {
    if let Some(file) = file {
        let text = std::fs::read_to_string(file).ctx(file)?;
        return Ok(text.trim_end_matches('\n').to_owned());
    }
    eprint!("{prompt}: ");
    let off = std::process::Command::new("stty").arg("-echo").status().is_ok_and(|status| status.success());
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    if off {
        let _ = std::process::Command::new("stty").arg("echo").status();
        eprintln!();
    }
    read.map_err(|error| Error(format!("reading the password: {error}")))?;
    Ok(line.trim_end_matches('\n').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("autark-pkg-repo-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Serves the files in `dir` over HTTP on a free port until the test process ends; 404 for anything else.
    fn serve(dir: PathBuf) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
                let name = line.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_owned();
                let mut body = Vec::new();
                let found = !name.contains("..") && std::fs::File::open(dir.join(&name)).and_then(|mut file| file.read_to_end(&mut body)).is_ok();
                let head = if found { format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()) } else { "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned() };
                let _ = stream.write_all(head.as_bytes());
                if found {
                    let _ = stream.write_all(&body);
                }
            }
        });
        format!("http://{address}")
    }

    /// A build system with a signed `hello 1.0-1` in its cache, and a release key pair.
    fn built(test: &str) -> (PathBuf, Config, PathBuf, PathBuf) {
        let top = scratch(test);
        let build = Config::load(&top.join("build"), None, None).unwrap();
        archive::keygen(&build).unwrap();
        let from = top.join("from");
        std::fs::create_dir_all(from.join("usr/share")).unwrap();
        std::fs::write(from.join("usr/share/hello.txt"), "hello\n").unwrap();
        std::fs::create_dir_all(&build.cache).unwrap();
        let info = PkgInfo { name: "hello".into(), version: "1.0-1".into(), depends: vec![], mode: "staged".into(), built: 100 };
        archive::pack(&build, &from, &["/usr".into(), "/usr/share".into(), "/usr/share/hello.txt".into()], &info, true, &build.cache.join(info.file_name())).unwrap();
        let (secret, public) = archive::release_keygen(&top.join("release"), "correct horse").unwrap();
        (top, build, secret, public)
    }

    #[test]
    fn a_published_package_is_downloaded_and_checked_against_the_release_key() {
        let (top, build, secret, public) = built("published");
        let out = top.join("out");
        assert_eq!(publish(&build, &out, &secret, "correct horse", &[]).unwrap(), 1);
        let index = std::fs::read_to_string(out.join("index")).unwrap();
        assert!(index.starts_with("hello 1.0-1 ") && index.trim_end().ends_with(" hello-1.0-1.tar.zst"), "{index}");
        assert!(out.join("index.minisig").exists());

        let mut client = Config::load(&top.join("client"), None, None).unwrap();
        client.repo = Some(serve(out));
        std::fs::create_dir_all(&client.keys).unwrap();

        // Not trusting the release key: refused, and nothing is kept.
        let error = prebuilt(&client, "hello", "1.0-1").unwrap_err().to_string();
        assert!(error.contains("refused"), "{error}");
        assert!(!client.cache.join("hello-1.0-1.tar.zst").exists());

        std::fs::copy(&public, client.keys.join("release.pub")).unwrap();
        let path = prebuilt(&client, "hello", "1.0-1").unwrap().unwrap();
        assert_eq!(path, client.cache.join("hello-1.0-1.tar.zst"));
        assert_eq!(archive::verify(&client, &path).unwrap().name, "hello");

        // A version the repository doesn't have: nothing, so the caller builds.
        assert!(prebuilt(&client, "hello", "2.0-1").unwrap().is_none());
        // No repository set: nothing.
        client.repo = None;
        assert!(prebuilt(&client, "hello", "1.0-1").unwrap().is_none());
    }

    #[test]
    fn the_build_key_does_not_vouch_for_a_download_and_a_wrong_password_signs_nothing() {
        let (top, build, secret, _public) = built("buildkey");
        let out = top.join("out");
        let error = publish(&build, &out, &secret, "wrong", &[]).unwrap_err().to_string();
        assert!(error.contains("wrong password"), "{error}");

        // Served unmodified from the build cache, signed by the build key only.
        let mut client = Config::load(&top.join("client"), None, None).unwrap();
        client.repo = Some(serve(build.cache.clone()));
        std::fs::create_dir_all(&client.keys).unwrap();
        let error = prebuilt(&client, "hello", "1.0-1").unwrap_err().to_string();
        assert!(error.contains("refused"), "{error}");
    }

    #[test]
    fn an_archive_served_under_another_name_is_refused() {
        let (top, build, secret, public) = built("othername");
        let out = top.join("out");
        publish(&build, &out, &secret, "correct horse", &[]).unwrap();
        // The same validly signed archive, offered as version 9.0-1.
        std::fs::copy(out.join("hello-1.0-1.tar.zst"), out.join("hello-9.0-1.tar.zst")).unwrap();
        std::fs::copy(out.join("hello-1.0-1.tar.zst.minisig"), out.join("hello-9.0-1.tar.zst.minisig")).unwrap();
        let mut client = Config::load(&top.join("client"), None, None).unwrap();
        client.repo = Some(serve(out));
        std::fs::create_dir_all(&client.keys).unwrap();
        std::fs::copy(&public, client.keys.join("release.pub")).unwrap();
        let error = prebuilt(&client, "hello", "9.0-1").unwrap_err().to_string();
        assert!(error.contains("refused") || error.contains("not hello 9.0-1"), "{error}");
        assert!(!client.cache.join("hello-9.0-1.tar.zst").exists());
    }
}
