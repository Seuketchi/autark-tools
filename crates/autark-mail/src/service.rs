//! `org.autark.Mail1`: mail for Kith, served by the app or by `autark-mail daemon`. Reading is open to the session; sending asks nothing
//! here because Kith asks you first, on a card showing the whole message, before it calls.

use autark_pim::mime::{Address, Outgoing};
use serde_json::json;

use crate::{backend, logic};

pub const PATH: &str = "/org/autark/Mail1";

pub struct Service;

fn failed(why: String) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(why)
}

fn account(id: &str) -> zbus::fdo::Result<autark_accounts::model::Account> {
    backend::accounts().map_err(failed)?.into_iter().find(|account| account.id == id).ok_or_else(|| failed(format!("no mail account called “{id}”")))
}

#[zbus::interface(name = "org.autark.Mail1")]
impl Service {
    /// The mail accounts, as JSON: `[{"id","name","email"}]`.
    async fn accounts(&self) -> zbus::fdo::Result<String> {
        let list = blocking::unblock(backend::accounts).await.map_err(failed)?;
        Ok(json!(list.iter().map(|account| json!({ "id": account.id, "name": account.name, "email": account.email })).collect::<Vec<_>>()).to_string())
    }

    /// The newest `count` messages in a folder (`INBOX`), newest first, as JSON:
    /// `[{"uid","from","subject","date","unread"}]`.
    async fn recent(&self, account: String, folder: String, count: u32) -> zbus::fdo::Result<String> {
        let account = account_by(&account).await?;
        let folder = if folder.trim().is_empty() { "INBOX".to_owned() } else { folder };
        let today = chrono::Local::now().date_naive();
        let list = blocking::unblock(move || backend::summaries(&account, &folder, count.clamp(1, 100) as usize)).await.map_err(failed)?;
        Ok(json!(list.iter().map(|m| json!({ "uid": m.uid, "from": m.from, "subject": m.subject, "date": logic::short_date(&m.date, today), "unread": !m.is_seen() })).collect::<Vec<_>>()).to_string())
    }

    /// One message as JSON: `{"from","to","subject","date","text","attachments"}`. The text is cut at 8 KB.
    /// Reading marks it read.
    async fn read(&self, account: String, folder: String, uid: u32) -> zbus::fdo::Result<String> {
        let account = account_by(&account).await?;
        let folder = if folder.trim().is_empty() { "INBOX".to_owned() } else { folder };
        let message = blocking::unblock(move || backend::read(&account, &folder, uid)).await.map_err(failed)?;
        Ok(message_json(&message))
    }

    /// The same as `Read`, but the message stays unread: for Kith, which looks without changing anything.
    async fn peek(&self, account: String, folder: String, uid: u32) -> zbus::fdo::Result<String> {
        let account = account_by(&account).await?;
        let folder = if folder.trim().is_empty() { "INBOX".to_owned() } else { folder };
        let message = blocking::unblock(move || backend::peek(&account, &folder, uid)).await.map_err(failed)?;
        Ok(message_json(&message))
    }

    /// Sends a plain-text message from `account`. `to` and `cc` are address lists such as `a@x.org, b@y.org`.
    async fn send(&self, account: String, to: String, cc: String, subject: String, body: String) -> zbus::fdo::Result<()> {
        let account = account_by(&account).await?;
        let (to, cc) = (logic::addresses(&to), logic::addresses(&cc));
        if to.is_empty() {
            return Err(failed("add someone to send it to".to_owned()));
        }
        let message = Outgoing {
            from: Address { name: String::new(), email: account.email.clone() },
            to,
            cc,
            subject,
            text: body,
            message_id: logic::message_id(&crate::compose::new_id(), &account.email),
            ..Default::default()
        };
        blocking::unblock(move || backend::send(&account, message)).await.map_err(failed)
    }
}

async fn account_by(id: &str) -> zbus::fdo::Result<autark_accounts::model::Account> {
    let id = id.to_owned();
    blocking::unblock(move || account(&id)).await
}

fn message_json(message: &autark_pim::mime::Message) -> String {
    let mut text = logic::body_text(message);
    if text.len() > 8 * 1024 {
        let mut end = 8 * 1024;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n(the message goes on)");
    }
    let names = |list: &[Address]| list.iter().map(|a| if a.name.is_empty() { a.email.clone() } else { format!("{} <{}>", a.name, a.email) }).collect::<Vec<_>>();
    json!({ "from": names(&message.from), "to": names(&message.to), "subject": message.subject, "date": message.date, "text": text, "attachments": message.attachments.iter().map(|a| a.name.clone()).collect::<Vec<_>>() }).to_string()
}
