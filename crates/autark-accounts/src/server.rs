//! `org.autark.Accounts1`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use zbus::message::Header;

use crate::model::{self, Account, Auth};
use crate::{oauth, secrets};

pub const PATH: &str = "/org/autark/Accounts1";

/// The programs that may ask for credentials. Anything else running as you could read the keyring directly,
/// so this is a fence against mistakes, not against malware; Kith is left off it on purpose.
const ASKERS: [&str; 6] = ["autark-calendar", "autark-tasks", "autark-contacts", "autark-mail", "autark-settings", "autark-accounts"];

pub fn may_ask(program: &str) -> bool {
    ASKERS.contains(&program)
}

pub struct Accounts {
    /// Access tokens made from refresh tokens, until a minute before they stop working.
    tokens: Mutex<HashMap<String, (String, Instant)>>,
}

impl Accounts {
    pub fn new() -> Self {
        Accounts { tokens: Mutex::new(HashMap::new()) }
    }

    fn cached(&self, id: &str) -> Option<String> {
        let tokens = self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tokens.get(id).filter(|(_, until)| *until > Instant::now()).map(|(token, _)| token.clone())
    }

    fn keep(&self, id: &str, access: &oauth::Access) {
        let until = Instant::now() + Duration::from_secs(access.expires_in.saturating_sub(60));
        self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(id.to_owned(), (access.token.clone(), until));
    }
}

fn failed(why: String) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(why)
}

/// The program on the other end of the bus connection, by its executable's name.
async fn program_of(connection: &zbus::Connection, header: &Header<'_>) -> Result<String, String> {
    let sender = header.sender().ok_or("no sender")?;
    let bus = zbus::fdo::DBusProxy::new(connection).await.map_err(|err| err.to_string())?;
    let pid = bus.get_connection_unix_process_id(sender.to_owned().into()).await.map_err(|err| err.to_string())?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|err| format!("who is asking? {err}"))?;
    Ok(exe.file_name().map(|name| name.to_string_lossy().trim_end_matches(" (deleted)").to_owned()).unwrap_or_default())
}

fn open_in_browser(url: &str) {
    if std::process::Command::new("xdg-open").arg(url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_err() {
        eprintln!("accounts: open this page to sign in: {url}");
    }
}

#[zbus::interface(name = "org.autark.Accounts1")]
impl Accounts {
    async fn list(&self) -> String {
        serde_json::to_string(&model::load().account).unwrap_or_else(|_| "[]".to_owned())
    }

    async fn add_oauth(&self, #[zbus(connection)] connection: &zbus::Connection, provider: String) -> zbus::fdo::Result<String> {
        let file = model::load();
        let client = match provider.as_str() {
            "google" => file.google.clone(),
            other => return Err(failed(format!("OAuth sign-in isn't set up for “{other}”"))),
        };
        let spec = oauth::provider(&provider).map_err(failed)?;
        let (access, refresh) = blocking::unblock(move || oauth::sign_in(spec, &client, open_in_browser)).await.map_err(failed)?;
        let token = access.token.clone();
        let email = blocking::unblock(move || oauth::email_of(spec, &token)).await.map_err(failed)?;
        // Read the file again: it may have changed while the browser was open.
        let mut file = model::load();
        if let Some(existing) = file.account.iter().find(|account| account.provider == provider && account.email.eq_ignore_ascii_case(&email)) {
            secrets::store(connection, &existing.id.clone(), "refresh_token", &format!("{email} (Autark)"), &refresh).await.map_err(failed)?;
            return Ok(existing.id.clone());
        }
        let mut account = model::preset(&provider, &email).map_err(failed)?;
        account.id = model::make_id(&file, &provider, &email);
        secrets::store(connection, &account.id, "refresh_token", &format!("{email} (Autark)"), &refresh).await.map_err(failed)?;
        self.keep(&account.id, &access);
        let id = account.id.clone();
        file.account.push(account);
        model::save(&file).map_err(failed)?;
        Ok(id)
    }

    async fn add_password(&self, #[zbus(connection)] connection: &zbus::Connection, provider: String, email: String, password: String, extra: String) -> zbus::fdo::Result<String> {
        let mut account = model::preset(&provider, &email).map_err(failed)?;
        if account.auth != Auth::Password {
            return Err(failed(format!("{provider} signs in with OAuth, not a password")));
        }
        if !extra.trim().is_empty() {
            let extra: Value = serde_json::from_str(&extra).map_err(|err| failed(format!("the extra settings aren't JSON: {err}")))?;
            for (key, field) in [("name", &mut account.name), ("login", &mut account.login), ("imap", &mut account.imap), ("smtp", &mut account.smtp), ("caldav", &mut account.caldav), ("carddav", &mut account.carddav)] {
                if let Some(value) = extra[key].as_str() {
                    *field = value.trim().to_owned();
                }
            }
        }
        if !account.has_mail() && account.caldav.is_empty() && account.carddav.is_empty() {
            return Err(failed("an account needs a mail server or a calendar or contacts address".to_owned()));
        }
        let mut file = model::load();
        account.id = model::make_id(&file, &provider, &email);
        secrets::store(connection, &account.id, "password", &format!("{email} (Autark)"), &password).await.map_err(failed)?;
        let id = account.id.clone();
        file.account.push(account);
        model::save(&file).map_err(failed)?;
        Ok(id)
    }

    async fn remove(&self, #[zbus(connection)] connection: &zbus::Connection, id: String) -> zbus::fdo::Result<()> {
        let mut file = model::load();
        let before = file.account.len();
        file.account.retain(|account| account.id != id);
        if file.account.len() == before {
            return Err(failed(format!("no account called “{id}”")));
        }
        model::save(&file).map_err(failed)?;
        self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&id);
        secrets::remove(connection, &id).await.map_err(failed)
    }

    async fn credentials(&self, #[zbus(connection)] connection: &zbus::Connection, #[zbus(header)] header: Header<'_>, id: String) -> zbus::fdo::Result<(String, String, String)> {
        let program = program_of(connection, &header).await.map_err(failed)?;
        if !may_ask(&program) {
            return Err(zbus::fdo::Error::AccessDenied(format!("{program} may not ask for account credentials")));
        }
        let file = model::load();
        let account: Account = file.account.iter().find(|account| account.id == id).cloned().ok_or_else(|| failed(format!("no account called “{id}”")))?;
        match account.auth {
            Auth::Password => {
                let password = secrets::read(connection, &id, "password").await.map_err(failed)?;
                Ok(("password".to_owned(), account.user().to_owned(), password))
            }
            Auth::OAuth => {
                if let Some(token) = self.cached(&id) {
                    return Ok(("oauth".to_owned(), account.user().to_owned(), token));
                }
                let refresh = secrets::read(connection, &id, "refresh_token").await.map_err(failed)?;
                let spec = oauth::provider(&account.provider).map_err(failed)?;
                let client = if account.provider == "google" { file.google } else { file.microsoft };
                let access = blocking::unblock(move || oauth::refresh(spec, &client, &refresh)).await.map_err(failed)?;
                self.keep(&id, &access);
                Ok(("oauth".to_owned(), account.user().to_owned(), access.token))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_autarks_own_apps_may_ask() {
        for program in ["autark-mail", "autark-calendar", "autark-contacts", "autark-tasks", "autark-settings"] {
            assert!(may_ask(program), "{program}");
        }
        for program in ["autark-ai", "python3", "curl", ""] {
            assert!(!may_ask(program), "{program}");
        }
    }
}
