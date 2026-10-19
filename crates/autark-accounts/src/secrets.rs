//! The Secret Service keyring (oo7-daemon on Autark, GNOME Keyring elsewhere), spoken to directly over D-Bus
//! in the "plain" algorithm, which only this user's processes can reach. Each secret is filed under
//! `application = autark-accounts`, `account = <id>`, `kind = password` or `refresh_token`.

use std::collections::HashMap;

use futures_util::StreamExt;
use zbus::{OwnedObjectPath, OwnedVariant as OwnedValue, Variant as Value};

const SERVICE: &str = "org.freedesktop.secrets";

fn attributes<'a>(account: &'a str, kind: &'a str) -> HashMap<&'static str, &'a str> {
    HashMap::from([("application", "autark-accounts"), ("account", account), ("kind", kind)])
}

async fn proxy<'a>(connection: &zbus::Connection, path: &'a str, interface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::Proxy::new(connection, SERVICE, path, interface).await
}

/// The secret filed for `account`, if there is one and the keyring is unlocked.
pub async fn read(connection: &zbus::Connection, account: &str, kind: &str) -> Result<String, String> {
    read_inner(connection, account, kind).await.map_err(|err| format!("the keyring: {err}"))
}

async fn read_inner(connection: &zbus::Connection, account: &str, kind: &str) -> zbus::Result<String> {
    let service = proxy(connection, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service.call("SearchItems", &(attributes(account, kind),)).await?;
    let Some(item) = unlocked.first() else {
        let why = if locked.is_empty() { "no secret saved for this account" } else { "the keyring is locked" };
        return Err(zbus::Error::Failure(why.to_owned()));
    };
    let (_, session): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Value::from(""))).await?;
    let item = proxy(connection, item.as_str(), "org.freedesktop.Secret.Item").await?;
    let secret: zbus::Result<(OwnedObjectPath, Vec<u8>, Vec<u8>, String)> = item.call("GetSecret", &(&session,)).await;
    if let Ok(session) = proxy(connection, session.as_str(), "org.freedesktop.Secret.Session").await {
        let _ = session.call_noreply("Close", &()).await;
    }
    String::from_utf8(secret?.2).map_err(|_| zbus::Error::Failure("the saved secret isn't text".to_owned()))
}

/// Files `secret` for `account`, replacing the one there. The keyring may ask for its own password first;
/// dismissing that is an error.
pub async fn store(connection: &zbus::Connection, account: &str, kind: &str, label: &str, secret: &str) -> Result<(), String> {
    store_inner(connection, account, kind, label, secret).await.map_err(|err| format!("the keyring: {err}"))
}

async fn store_inner(connection: &zbus::Connection, account: &str, kind: &str, label: &str, secret: &str) -> zbus::Result<()> {
    let service = proxy(connection, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let (_, session): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Value::from(""))).await?;
    let result = async {
        let collection: OwnedObjectPath = service.call("ReadAlias", &("default",)).await?;
        if collection.as_str() == "/" {
            return Err(zbus::Error::Failure("there is no default keyring; create one first".to_owned()));
        }
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Unlock", &(vec![collection.clone()],)).await?;
        if prompted(connection, &prompt).await?.is_none() {
            return Err(zbus::Error::Failure("the keyring was not unlocked".to_owned()));
        }
        let properties = HashMap::from([("org.freedesktop.Secret.Item.Label", Value::from(label)), ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes(account, kind)))]);
        let secret = (session.clone(), Vec::<u8>::new(), secret.as_bytes(), "text/plain");
        let collection = proxy(connection, collection.as_str(), "org.freedesktop.Secret.Collection").await?;
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = collection.call("CreateItem", &(properties, secret, true)).await?;
        prompted(connection, &prompt).await?.map(drop).ok_or_else(|| zbus::Error::Failure("the keyring did not save it".to_owned()))
    }
    .await;
    if let Ok(session) = proxy(connection, session.as_str(), "org.freedesktop.Secret.Session").await {
        let _ = session.call_noreply("Close", &()).await;
    }
    result
}

/// Forgets what is filed for `account` (every kind), if anything is.
pub async fn remove(connection: &zbus::Connection, account: &str) -> Result<(), String> {
    remove_inner(connection, account).await.map_err(|err| format!("the keyring: {err}"))
}

async fn remove_inner(connection: &zbus::Connection, account: &str) -> zbus::Result<()> {
    let service = proxy(connection, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let attributes = HashMap::from([("application", "autark-accounts"), ("account", account)]);
    let (unlocked, _): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service.call("SearchItems", &(attributes,)).await?;
    for item in unlocked {
        let item = proxy(connection, item.as_str(), "org.freedesktop.Secret.Item").await?;
        let prompt: OwnedObjectPath = item.call("Delete", &()).await?;
        prompted(connection, &prompt).await?;
    }
    Ok(())
}

/// Shows the keyring's prompt, if it has one, and waits for it: `None` when it was dismissed.
async fn prompted(connection: &zbus::Connection, prompt: &OwnedObjectPath) -> zbus::Result<Option<OwnedValue>> {
    if prompt.as_str() == "/" {
        return Ok(Some(OwnedValue::from(0u8)));
    }
    let prompt = proxy(connection, prompt.as_str(), "org.freedesktop.Secret.Prompt").await?;
    let mut completed = prompt.receive_signal("Completed").await?;
    prompt.call::<_, _, ()>("Prompt", &("",)).await?;
    let Some(message) = completed.next().await else { return Ok(None) };
    let (dismissed, result): (bool, OwnedValue) = message.body().deserialize()?;
    Ok((!dismissed).then_some(result))
}
