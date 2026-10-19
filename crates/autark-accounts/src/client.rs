//! The accounts service as the apps see it.

#[zbus::proxy(interface = "org.autark.Accounts1", default_service = "org.autark.Accounts1", default_path = "/org/autark/Accounts1")]
pub trait Accounts {
    /// The accounts as a JSON array (no secrets): see `model::Account`.
    fn list(&self) -> zbus::Result<String>;

    /// Signs in to a provider that uses OAuth (`google`) in the browser, and returns the new account's ID.
    fn add_oauth(&self, provider: &str) -> zbus::Result<String>;

    /// Adds an account that signs in with a password. `extra` is JSON with any of `name`, `login`, `imap`,
    /// `smtp`, `caldav` and `carddav` to override the provider's servers. Returns the ID.
    fn add_password(&self, provider: &str, email: &str, password: &str, extra: &str) -> zbus::Result<String>;

    fn remove(&self, id: &str) -> zbus::Result<()>;

    /// What an app signs in with: the kind (`password` or `oauth`), the name to sign in as, and the password
    /// or a fresh access token. Only Autark's own apps are answered.
    fn credentials(&self, id: &str) -> zbus::Result<(String, String, String)>;
}
