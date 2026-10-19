//! `org.autark.Contacts1`: the address book for Kith and for Mail's address completion while the app runs.
//! Changing asks nothing here because Kith asks you first, on a card, before it calls.

use std::sync::Arc;

use serde_json::json;

use crate::backend::Backend;
use crate::logic;

pub const PATH: &str = "/org/autark/Contacts1";

pub struct Service {
    pub backend: Arc<Backend>,
}

fn failed(why: String) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(why)
}

#[zbus::interface(name = "org.autark.Contacts1")]
impl Service {
    /// The address books, as JSON: `[{"account","name","url"}]`.
    async fn books(&self) -> String {
        json!(self.backend.snapshot().iter().map(|book| json!({ "account": book.account_name, "name": book.name, "url": book.url })).collect::<Vec<_>>()).to_string()
    }

    /// Up to `limit` contacts matching `query` (every word in the name, organisation, address or number), as
    /// JSON: `[{"name","organisation","emails","phones","resource"}]`. Fetches first if nothing is loaded.
    async fn search(&self, query: String, limit: u32) -> zbus::fdo::Result<String> {
        if self.backend.snapshot().is_empty() {
            let backend = self.backend.clone();
            let problems = blocking::unblock(move || backend.refresh()).await;
            if let Some(why) = problems.first().filter(|_| self.backend.snapshot().is_empty()) {
                return Err(failed(why.clone()));
            }
        }
        let mut found: Vec<_> = self.backend.snapshot().into_iter().flat_map(|book| book.contacts).filter(|stored| logic::matches(&stored.contact, &query)).collect();
        found.sort_by_key(|stored| logic::sort_key(&stored.contact));
        found.truncate(limit.clamp(1, 200) as usize);
        let list: Vec<_> = found
            .iter()
            .map(|stored| json!({ "name": logic::title(&stored.contact), "organisation": stored.contact.organisation, "emails": stored.contact.emails, "phones": stored.contact.phones, "resource": stored.url }))
            .collect();
        Ok(json!(list).to_string())
    }

    /// Adds a contact to the address book at `book` (its `url` from `Books`). Returns the contact's ID.
    async fn create(&self, book: String, name: String, email: String, phone: String, organisation: String) -> zbus::fdo::Result<String> {
        if name.trim().is_empty() && email.trim().is_empty() {
            return Err(failed("a contact needs a name or an address".to_owned()));
        }
        if !email.trim().is_empty() && !logic::valid_email(email.trim()) {
            return Err(failed(format!("“{email}” isn't an email address")));
        }
        let uid = crate::editor::new_uid();
        let contact = logic::build(&uid, &name, &organisation, &email, &phone, "");
        let found = self.backend.snapshot().into_iter().find(|candidate| candidate.url == book).ok_or_else(|| failed("no such address book".to_owned()))?;
        let backend = self.backend.clone();
        blocking::unblock(move || backend.create(&found.account, &found.url, contact)).await.map_err(failed)?;
        Ok(uid)
    }

    /// Deletes the contact whose `resource` a search gave.
    async fn delete(&self, resource: String) -> zbus::fdo::Result<()> {
        let backend = self.backend.clone();
        blocking::unblock(move || backend.delete(&resource)).await.map_err(failed)
    }
}
