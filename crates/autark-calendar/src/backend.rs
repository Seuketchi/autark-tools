//! The calendars and what is in them, kept in step with each account's CalDAV server. Nothing here is a
//! widget, so the window and the D-Bus service (for Kith) both use it, from any thread; its calls wait on the
//! network, so callers run them off the main thread.

use std::sync::Mutex;

use autark_accounts::client::AccountsProxy;
use autark_accounts::model::Account;
use autark_pim::dav::{Auth, Collection, CollectionKind, Dav, Resource};
use autark_pim::ical::{self, Calendar as Parsed, Item};
use chrono::NaiveDate;

/// A resource as the server has it, and read.
#[derive(Clone, Debug)]
pub struct Stored {
    pub url: String,
    pub etag: String,
    pub parsed: Parsed,
}

/// A calendar on an account.
#[derive(Clone, Debug)]
pub struct Cal {
    pub account: String,
    pub account_name: String,
    pub url: String,
    pub name: String,
    pub color: String,
    pub resources: Vec<Stored>,
}

#[derive(Default)]
pub struct Backend {
    calendars: Mutex<Vec<Cal>>,
}

/// Credentials from the accounts service, as a CalDAV login.
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

fn utc(day: NaiveDate) -> String {
    day.format("%Y%m%dT000000Z").to_string()
}

impl Backend {
    pub fn snapshot(&self) -> Vec<Cal> {
        self.calendars.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn with<T>(&self, change: impl FnOnce(&mut Vec<Cal>) -> T) -> T {
        change(&mut self.calendars.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Fetches the events from `from` up to (not including) `to` for every account with a calendar. One
    /// account failing doesn't stop the rest; the failures come back as text, and what was fetched earlier for
    /// that account stays.
    pub fn refresh(&self, from: NaiveDate, to: NaiveDate) -> Vec<String> {
        let accounts = match accounts() {
            Ok(accounts) => accounts,
            Err(why) => return vec![why],
        };
        let mut problems = Vec::new();
        let mut fresh: Vec<(String, Vec<Cal>)> = Vec::new();
        for account in accounts.iter().filter(|account| !account.caldav.is_empty()) {
            match fetch(account, from, to) {
                Ok(calendars) => fresh.push((account.id.clone(), calendars)),
                Err(why) => problems.push(why),
            }
        }
        let known: Vec<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
        self.with(|calendars| {
            calendars.retain(|cal| known.contains(&cal.account.as_str()) && !fresh.iter().any(|(id, _)| *id == cal.account));
            for (_, new) in fresh {
                calendars.extend(new);
            }
            calendars.sort_by(|a, b| a.account_name.cmp(&b.account_name).then(a.name.cmp(&b.name)));
        });
        problems
    }

    /// A stored item, with the calendar it is in.
    pub fn find(&self, resource: &str, uid: &str) -> Option<(Cal, Item)> {
        self.snapshot().into_iter().find_map(|cal| {
            let item = cal.resources.iter().find(|stored| stored.url == resource)?.parsed.items.iter().find(|item| item.uid == uid && item.recurrence_id.is_none())?.clone();
            Some((cal, item))
        })
    }

    fn dav_for(&self, account_id: &str) -> Result<Dav, String> {
        let account = accounts()?.into_iter().find(|account| account.id == account_id).ok_or_else(|| format!("no account called “{account_id}”"))?;
        Ok(Dav::new(credentials(&account)?))
    }

    /// Adds `item` to the calendar at `calendar_url`. The item's UID names the file, so it never replaces another.
    pub fn create(&self, account_id: &str, calendar_url: &str, item: Item) -> Result<(), String> {
        let dav = self.dav_for(account_id)?;
        let url = format!("{}/{}.ics", calendar_url.trim_end_matches('/'), item.uid);
        let parsed = Parsed { prelude: Vec::new(), items: vec![item] };
        let etag = dav.put(&url, &ical::write(&parsed), "text/calendar; charset=utf-8", None)?;
        self.with(|calendars| {
            if let Some(cal) = calendars.iter_mut().find(|cal| cal.account == account_id && cal.url == calendar_url) {
                cal.resources.push(Stored { url, etag, parsed });
            }
        });
        Ok(())
    }

    /// Replaces the event `item.uid` in the resource at `resource_url`, keeping what the item doesn't say
    /// (alarms, guests, the other showings of a series).
    pub fn update(&self, resource_url: &str, item: Item) -> Result<(), String> {
        let (cal, mut stored) = self.stored(resource_url)?;
        let dav = self.dav_for(&cal.account)?;
        let slot = stored.parsed.items.iter_mut().find(|old| old.uid == item.uid && old.recurrence_id.is_none()).ok_or("that event is no longer there")?;
        let mut new = item;
        // Lines the editor never reads stay as they were.
        new.extra = slot.extra.clone();
        new.exdates = slot.exdates.clone();
        new.rrule = slot.rrule.clone();
        *slot = new;
        let etag = dav.put(resource_url, &ical::write(&stored.parsed), "text/calendar; charset=utf-8", Some(&stored.etag))?;
        stored.etag = etag;
        self.with(|calendars| {
            if let Some(old) = calendars.iter_mut().flat_map(|cal| cal.resources.iter_mut()).find(|old| old.url == resource_url) {
                *old = stored;
            }
        });
        Ok(())
    }

    /// Deletes the whole resource (all showings of a repeating event).
    pub fn delete(&self, resource_url: &str) -> Result<(), String> {
        let (cal, stored) = self.stored(resource_url)?;
        self.dav_for(&cal.account)?.delete(resource_url, &stored.etag)?;
        self.with(|calendars| {
            for cal in calendars.iter_mut() {
                cal.resources.retain(|old| old.url != resource_url);
            }
        });
        Ok(())
    }

    fn stored(&self, resource_url: &str) -> Result<(Cal, Stored), String> {
        self.snapshot()
            .into_iter()
            .find_map(|cal| {
                let stored = cal.resources.iter().find(|stored| stored.url == resource_url)?.clone();
                Some((cal, stored))
            })
            .ok_or_else(|| "that event is no longer there".to_owned())
    }
}

fn fetch(account: &Account, from: NaiveDate, to: NaiveDate) -> Result<Vec<Cal>, String> {
    let dav = Dav::new(credentials(account)?);
    let mut collections: Vec<Collection> = dav.discover(&account.caldav).unwrap_or_default().into_iter().filter(|collection| collection.kind == CollectionKind::Calendar).collect();
    collections.retain(|collection| collection.components.is_empty() || collection.components.iter().any(|name| name == "VEVENT"));
    // A server address that is itself a calendar (Google's) has nothing to discover.
    if collections.is_empty() {
        collections.push(Collection { url: account.caldav.clone(), name: account.name.clone(), kind: CollectionKind::Calendar, components: Vec::new(), color: String::new() });
    }
    let mut calendars = Vec::new();
    for collection in collections {
        let resources: Vec<Resource> = dav.events(&collection.url, &utc(from), &utc(to)).map_err(|why| format!("{}: {why}", account.name))?;
        calendars.push(Cal {
            account: account.id.clone(),
            account_name: account.name.clone(),
            url: collection.url,
            name: collection.name,
            color: crate::logic::colour(&collection.color),
            resources: resources.into_iter().map(|resource| Stored { parsed: ical::parse(&resource.data), url: resource.url, etag: resource.etag }).collect(),
        });
    }
    Ok(calendars)
}
