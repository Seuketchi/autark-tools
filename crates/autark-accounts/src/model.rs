//! The accounts file, and what each kind of provider needs filled in.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How an account signs in.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    /// A password (an app password, for providers that ask for one).
    #[default]
    Password,
    /// OAuth 2: a refresh token in the keyring, access tokens made from it.
    OAuth,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Account {
    /// A short name that doesn't change, such as `ada-gmail`.
    pub id: String,
    /// What the apps show.
    pub name: String,
    /// `google`, `icloud`, `fastmail` or `generic`.
    pub provider: String,
    pub email: String,
    /// What to sign in as, when it isn't the address.
    pub login: String,
    pub auth: Auth,
    /// `host:port` for IMAP (TLS) and SMTP (465 is TLS, 587 is STARTTLS); empty if the account has no mail.
    pub imap: String,
    pub smtp: String,
    /// Server addresses to find calendars and address books from; empty if the account has none.
    pub caldav: String,
    pub carddav: String,
}

impl Account {
    /// The name to sign in as.
    pub fn user(&self) -> &str {
        if self.login.is_empty() { &self.email } else { &self.login }
    }

    pub fn has_mail(&self) -> bool {
        !self.imap.is_empty() && !self.smtp.is_empty()
    }
}

/// The OAuth client an app registered with a provider: Google and Microsoft give each app its own.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Client {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct File {
    pub account: Vec<Account>,
    pub google: Client,
    pub microsoft: Client,
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("autark/accounts.toml")
}

pub fn load() -> File {
    let path = path();
    let Ok(text) = std::fs::read_to_string(&path) else { return File::default() };
    toml::from_str(&text).unwrap_or_else(|err| {
        eprintln!("accounts: {}: {err}", path.display());
        File::default()
    })
}

/// Saves the file, readable only by you, replacing the old one in one step.
pub fn save(file: &File) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    let text = toml::to_string_pretty(file).map_err(|err| format!("accounts: {err}"))?;
    let temporary = path.with_extension("toml.new");
    std::fs::write(&temporary, text).map_err(|err| format!("{}: {err}", temporary.display()))?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).map_err(|err| format!("{}: {err}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|err| format!("{}: {err}", path.display()))
}

/// An id from the address and provider that no other account has: `ada-gmail`, `ada-gmail-2`.
pub fn make_id(file: &File, provider: &str, email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    let base: String = format!("{local}-{provider}").to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut id = base.clone();
    let mut n = 2;
    while file.account.iter().any(|account| account.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// A new account for `provider` with its servers filled in. `generic` leaves them for the caller.
pub fn preset(provider: &str, email: &str) -> Result<Account, String> {
    if !email.contains('@') || email.contains(char::is_whitespace) {
        return Err(format!("“{email}” isn't an email address"));
    }
    let (auth, imap, smtp, caldav, carddav) = match provider {
        // Google's CalDAV and CardDAV take an OAuth token; the calendar home is under the account's address.
        "google" => (Auth::OAuth, "imap.gmail.com:993", "smtp.gmail.com:465", format!("https://apidata.googleusercontent.com/caldav/v2/{email}/events/"), "https://www.googleapis.com/.well-known/carddav".to_owned()),
        "icloud" => (Auth::Password, "imap.mail.me.com:993", "smtp.mail.me.com:587", "https://caldav.icloud.com/".to_owned(), "https://contacts.icloud.com/".to_owned()),
        "fastmail" => (Auth::Password, "imap.fastmail.com:993", "smtp.fastmail.com:465", "https://caldav.fastmail.com/".to_owned(), "https://carddav.fastmail.com/".to_owned()),
        "generic" => (Auth::Password, "", "", String::new(), String::new()),
        other => return Err(format!("no provider called “{other}”")),
    };
    Ok(Account { provider: provider.to_owned(), email: email.to_owned(), auth, imap: imap.to_owned(), smtp: smtp.to_owned(), caldav, carddav, name: email.to_owned(), ..Default::default() })
}

/// Splits `host:port`, with `default` when there's no port.
pub fn host_port(text: &str, default: u16) -> Result<(String, u16), String> {
    match text.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Ok((host.to_owned(), port.parse().map_err(|_| format!("“{text}” has no port number"))?)),
        _ if !text.is_empty() => Ok((text.to_owned(), default)),
        _ => Err("no server is set for this account".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_fill_in_servers() {
        let google = preset("google", "ada@gmail.com").unwrap();
        assert_eq!((google.auth, google.imap.as_str()), (Auth::OAuth, "imap.gmail.com:993"));
        assert!(google.caldav.contains("ada@gmail.com"));
        let icloud = preset("icloud", "ada@icloud.com").unwrap();
        assert_eq!(icloud.auth, Auth::Password);
        assert!(preset("generic", "ada@x.org").unwrap().imap.is_empty());
        assert!(preset("aol", "a@b.c").is_err() && preset("google", "nope").is_err());
    }

    #[test]
    fn ids_are_unique_and_tidy() {
        let mut file = File::default();
        let first = make_id(&file, "google", "Ada.L@gmail.com");
        assert_eq!(first, "ada-l-google");
        file.account.push(Account { id: first, ..Default::default() });
        assert_eq!(make_id(&file, "google", "Ada.L@gmail.com"), "ada-l-google-2");
    }

    #[test]
    fn the_file_round_trips() {
        let mut file = File::default();
        file.google = Client { client_id: "id".into(), client_secret: "s".into() };
        file.account.push(preset("fastmail", "ada@fastmail.com").unwrap());
        let text = toml::to_string_pretty(&file).unwrap();
        assert_eq!(toml::from_str::<File>(&text).unwrap(), file);
        assert_eq!(host_port("imap.x.org:993", 143).unwrap(), ("imap.x.org".to_owned(), 993));
        assert_eq!(host_port("imap.x.org", 993).unwrap().1, 993);
        assert!(host_port("", 993).is_err());
    }
}
