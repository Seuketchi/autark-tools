//! The address books and what is in them, kept in step with each account's CardDAV server. Nothing here is a
//! widget, so the window and the D-Bus service (for Kith and Mail) share it from any thread; its calls wait on
//! the network, so callers run them off the main thread.

use std::sync::Mutex;

use autark_accounts::client::AccountsProxy;
use autark_accounts::model::Account;
use autark_pim::dav::{Auth, Collection, CollectionKind, Dav};
use autark_pim::vcard::{self, Contact};

/// One contact as the server has it.
#[derive(Clone, Debug)]
pub struct Stored {
    pub url: String,
    pub etag: String,
    pub contact: Contact,
}

#[derive(Clone, Debug)]
pub struct Book {
    pub account: String,
    pub account_name: String,
    pub url: String,
    pub name: String,
    pub contacts: Vec<Stored>,
}

#[derive(Default)]
pub struct Backend {
    books: Mutex<Vec<Book>>,
}

fn credentials(account: &Account) -> Result<Auth, String> {
    async_io::block_on(async {
        let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
        let accounts = AccountsProxy::new(&connection).await.map_err(|err| format!("the accounts service: {err}"))?;
        let (kind, user, secret) = accounts.credentials(&account.id).await.map_err(|err| format!("{}: {err}", account.name))?;
        Ok(if kind == "oauth" { Auth::Bearer(secret) } else { Auth::Basic { user, password: secret } })
    })
}

fn accounts() -> Result<Vec<Account>, String> {
    async_io::block_on(async {
        let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
        let accounts = AccountsProxy::new(&connection).await.map_err(|err| format!("the accounts service: {err}"))?;
        let text = accounts.list().await.map_err(|err| format!("the accounts service: {err}"))?;
        serde_json::from_str(&text).map_err(|err| format!("the accounts list: {err}"))
    })
}

impl Backend {
    pub fn snapshot(&self) -> Vec<Book> {
        self.books.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn with<T>(&self, change: impl FnOnce(&mut Vec<Book>) -> T) -> T {
        change(&mut self.books.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Fetches every address book of every account that has one. One account failing doesn't stop the rest; the
    /// failures come back as text, and what was fetched earlier for that account stays.
    pub fn refresh(&self) -> Vec<String> {
        let accounts = match accounts() {
            Ok(accounts) => accounts,
            Err(why) => return vec![why],
        };
        let mut problems = Vec::new();
        let mut fresh: Vec<(String, Vec<Book>)> = Vec::new();
        for account in accounts.iter().filter(|account| !account.carddav.is_empty()) {
            match fetch(account) {
                Ok(books) => fresh.push((account.id.clone(), books)),
                Err(why) => problems.push(why),
            }
        }
        let known: Vec<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
        self.with(|books| {
            books.retain(|book| known.contains(&book.account.as_str()) && !fresh.iter().any(|(id, _)| *id == book.account));
            for (_, new) in fresh {
                books.extend(new);
            }
            books.sort_by(|a, b| a.account_name.cmp(&b.account_name).then(a.name.cmp(&b.name)));
        });
        problems
    }

    fn dav_for(&self, account_id: &str) -> Result<Dav, String> {
        let account = accounts()?.into_iter().find(|account| account.id == account_id).ok_or_else(|| format!("no account called “{account_id}”"))?;
        Ok(Dav::new(credentials(&account)?))
    }

    fn stored(&self, resource: &str) -> Result<(Book, Stored), String> {
        self.snapshot()
            .into_iter()
            .find_map(|book| {
                let stored = book.contacts.iter().find(|stored| stored.url == resource)?.clone();
                Some((book, stored))
            })
            .ok_or_else(|| "that contact is no longer there".to_owned())
    }

    /// Adds `contact` to the address book at `book_url`; its UID names the file, so it never replaces another.
    pub fn create(&self, account_id: &str, book_url: &str, contact: Contact) -> Result<(), String> {
        let dav = self.dav_for(account_id)?;
        let url = format!("{}/{}.vcf", book_url.trim_end_matches('/'), contact.uid);
        let etag = dav.put(&url, &vcard::write(&contact), "text/vcard; charset=utf-8", None)?;
        self.with(|books| {
            if let Some(book) = books.iter_mut().find(|book| book.account == account_id && book.url == book_url) {
                book.contacts.push(Stored { url, etag, contact });
            }
        });
        Ok(())
    }

    /// Replaces the contact at `resource`, keeping the lines the editor doesn't show (photo, birthday, addresses).
    pub fn update(&self, resource: &str, mut contact: Contact) -> Result<(), String> {
        let (book, mut stored) = self.stored(resource)?;
        let dav = self.dav_for(&book.account)?;
        contact.uid = stored.contact.uid.clone();
        contact.extra = stored.contact.extra.clone();
        let etag = dav.put(resource, &vcard::write(&contact), "text/vcard; charset=utf-8", Some(&stored.etag))?;
        stored.etag = etag;
        stored.contact = contact;
        self.with(|books| {
            if let Some(old) = books.iter_mut().flat_map(|book| book.contacts.iter_mut()).find(|old| old.url == resource) {
                *old = stored;
            }
        });
        Ok(())
    }

    pub fn delete(&self, resource: &str) -> Result<(), String> {
        let (book, stored) = self.stored(resource)?;
        self.dav_for(&book.account)?.delete(resource, &stored.etag)?;
        self.with(|books| {
            for book in books.iter_mut() {
                book.contacts.retain(|old| old.url != resource);
            }
        });
        Ok(())
    }
}

fn fetch(account: &Account) -> Result<Vec<Book>, String> {
    let dav = Dav::new(credentials(account)?);
    let mut collections: Vec<Collection> = dav.discover(&account.carddav).unwrap_or_default().into_iter().filter(|collection| collection.kind == CollectionKind::AddressBook).collect();
    // A server address that is itself an address book has nothing to discover.
    if collections.is_empty() {
        collections.push(Collection { url: account.carddav.clone(), name: account.name.clone(), kind: CollectionKind::AddressBook, components: Vec::new(), color: String::new() });
    }
    let mut books = Vec::new();
    for collection in collections {
        let resources = dav.contacts(&collection.url).map_err(|why| format!("{}: {why}", account.name))?;
        let contacts = resources.into_iter().flat_map(|resource| vcard::parse(&resource.data).into_iter().map(move |contact| Stored { url: resource.url.clone(), etag: resource.etag.clone(), contact })).collect();
        books.push(Book { account: account.id.clone(), account_name: account.name.clone(), url: collection.url, name: collection.name, contacts });
    }
    Ok(books)
}
