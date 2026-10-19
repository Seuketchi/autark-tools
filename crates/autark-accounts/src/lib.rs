//! The accounts Autark's apps share: a Google, iCloud, Fastmail or other account is signed in once, here, and
//! Calendar, Tasks, Contacts and Mail ask for what they need (`org.autark.Accounts1`). Kith never does: it
//! reaches your mail and calendar through those apps, so it holds no provider credentials.
//!
//! Account details live in `~/.config/autark/accounts.toml`; passwords and OAuth refresh tokens live in the
//! Secret Service keyring and nowhere else.

pub mod client;
pub mod model;
pub mod oauth;
pub mod secrets;
pub mod server;
