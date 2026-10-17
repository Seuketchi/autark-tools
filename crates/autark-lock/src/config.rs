//! `lock.toml`: every field is optional.

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Lock {
    /// The PAM service under `/etc/pam.d/` that checks the password.
    pub pam_service: String,
    /// Lock before the laptop sleeps (with `--daemon`).
    pub lock_on_sleep: bool,
    /// Also unlock with an enrolled fingerprint, through fprintd.
    pub fingerprint: bool,
    /// Show what notifications say on the lock screen, not only which apps sent them.
    pub notification_text: bool,
}

impl Default for Lock {
    fn default() -> Self {
        Self { pam_service: "autark-lock".to_owned(), lock_on_sleep: true, fingerprint: true, notification_text: false }
    }
}
