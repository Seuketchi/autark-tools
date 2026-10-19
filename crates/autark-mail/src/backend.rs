//! Talking to the mail servers: folders, message lists, reading, deleting and sending, for every account that
//! has mail. Each call signs in afresh (a password or an OAuth token from the accounts service) and signs out
//! when done, so there is no connection to keep alive or lose. Nothing here is a widget; its calls wait on the
//! network, so callers run them off the main thread.

use autark_accounts::client::AccountsProxy;
use autark_accounts::model::{Account, host_port};
use autark_pim::imap::{self, Folder, Imap, Summary};
use autark_pim::mime::{self, Message, Outgoing};
use autark_pim::smtp::{self, Login};
use autark_pim::tls;

use crate::logic;

/// What an account signs in with: the kind (`password` or `oauth`), the name, and the password or token.
struct Credentials {
    oauth: bool,
    user: String,
    secret: String,
}

fn credentials(account: &Account) -> Result<Credentials, String> {
    async_io::block_on(async {
        let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
        let accounts = AccountsProxy::new(&connection).await.map_err(|err| format!("the accounts service: {err}"))?;
        let (kind, user, secret) = accounts.credentials(&account.id).await.map_err(|err| format!("{}: {err}", account.name))?;
        Ok(Credentials { oauth: kind == "oauth", user, secret })
    })
}

/// The accounts that have mail.
pub fn accounts() -> Result<Vec<Account>, String> {
    async_io::block_on(async {
        let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
        let accounts = AccountsProxy::new(&connection).await.map_err(|err| format!("the accounts service: {err}"))?;
        let text = accounts.list().await.map_err(|err| format!("the accounts service: {err}"))?;
        let all: Vec<Account> = serde_json::from_str(&text).map_err(|err| format!("the accounts list: {err}"))?;
        Ok(all.into_iter().filter(Account::has_mail).collect())
    })
}

fn open(account: &Account) -> Result<Imap<tls::Stream>, String> {
    let (host, port) = host_port(&account.imap, 993)?;
    let creds = credentials(account)?;
    let mut session = imap::connect(&host, port).map_err(|why| format!("{}: {why}", account.name))?;
    let signed_in = if creds.oauth { session.login_oauth(&creds.user, &creds.secret) } else { session.login(&creds.user, &creds.secret) };
    signed_in.map_err(|why| format!("{}: {why}", account.name))?;
    Ok(session)
}

/// The account's folders, in the order to list them.
pub fn folders(account: &Account) -> Result<Vec<Folder>, String> {
    let mut session = open(account)?;
    let folders = session.folders();
    session.logout();
    folders.map(logic::ordered)
}

/// The newest `count` messages in `folder`, newest first.
pub fn summaries(account: &Account, folder: &str, count: usize) -> Result<Vec<Summary>, String> {
    let mut session = open(account)?;
    let result = (|| {
        session.select(folder)?;
        let uids = session.search("ALL")?;
        let newest: Vec<u32> = uids.iter().rev().take(count).rev().copied().collect();
        let mut list = session.summaries(&newest)?;
        list.reverse();
        Ok(list)
    })();
    session.logout();
    result
}

/// Reads a message and marks it read.
pub fn read(account: &Account, folder: &str, uid: u32) -> Result<Message, String> {
    let mut session = open(account)?;
    let result = (|| {
        session.select(folder)?;
        let raw = session.fetch(uid)?;
        session.flag(uid, &["\\Seen"], true)?;
        Ok(mime::parse(&raw))
    })();
    session.logout();
    result
}

/// Reads a message and leaves it as it was: unread stays unread.
pub fn peek(account: &Account, folder: &str, uid: u32) -> Result<Message, String> {
    let mut session = open(account)?;
    let result = session.select(folder).and_then(|_| session.fetch(uid)).map(|raw| mime::parse(&raw));
    session.logout();
    result
}

pub fn mark(account: &Account, folder: &str, uid: u32, flag: &str, add: bool) -> Result<(), String> {
    let mut session = open(account)?;
    let result = session.select(folder).and_then(|_| session.flag(uid, &[flag], add));
    session.logout();
    result
}

/// Moves a message to the account's trash folder.
pub fn trash(account: &Account, folder: &str, uid: u32) -> Result<(), String> {
    let mut session = open(account)?;
    let result = (|| {
        let folders = session.folders()?;
        let target = logic::trash_of(&folders).ok_or("this account has no Trash folder")?.name.clone();
        if target == folder {
            return Err("it is already in the Trash".to_owned());
        }
        session.select(folder)?;
        session.move_to(uid, &target)
    })();
    session.logout();
    result
}

/// Sends `message` from `account`, then files a copy in Sent (Gmail does that itself).
pub fn send(account: &Account, mut message: Outgoing) -> Result<(), String> {
    if message.to.is_empty() && message.cc.is_empty() {
        return Err("add someone to send it to".to_owned());
    }
    let (host, port) = host_port(&account.smtp, 465)?;
    let creds = credentials(account)?;
    message.date = chrono::Local::now().to_rfc2822();
    let recipients: Vec<String> = message.to.iter().chain(&message.cc).map(|address| address.email.clone()).collect();
    let text = mime::build(&message);
    let login = if creds.oauth { Login::OAuth { user: creds.user.clone(), token: creds.secret.clone() } } else { Login::Password { user: creds.user.clone(), password: creds.secret.clone() } };
    smtp::send(&host, port, &login, &message.from.email, &recipients, &text).map_err(|why| format!("{}: {why}", account.name))?;
    if account.provider != "google" {
        // The message has gone; a failure to file the copy is worth saying, not failing over.
        let filed = (|| {
            let mut session = open(account)?;
            let result = session.folders().and_then(|folders| {
                let sent = logic::sent_of(&folders).ok_or("no Sent folder")?.name.clone();
                session.append(&sent, &text)
            });
            session.logout();
            result
        })();
        if let Err(why) = filed {
            return Err(format!("sent, but no copy was saved: {why}"));
        }
    }
    Ok(())
}
