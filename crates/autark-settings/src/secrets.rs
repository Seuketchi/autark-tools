//! The Secret Service keyring (oo7-daemon on Autark, GNOME Keyring elsewhere), spoken to directly over
//! D-Bus: Settings only stores Jev's API key and asks whether one is there, never reads it back. The key
//! goes over the session bus in the "plain" algorithm, which only this user's processes can reach.

use std::collections::HashMap;

use futures_util::StreamExt;
use zbus::{OwnedObjectPath, OwnedVariant as OwnedValue, Variant as Value};
use zeroize::Zeroizing;

const SERVICE: &str = "org.freedesktop.secrets";
pub const DAEMON: &str = "The keyring (Secret Service)";

/// What Jev's key is filed under, for `autark-ai` to find.
fn attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("application", "autark-ai"), ("key", "jev")])
}

async fn proxy<'a>(connection: &zbus::Connection, path: &'a str, interface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::Proxy::new(connection, SERVICE, path, interface).await
}

/// Whether Jev's key is in the keyring, locked or not.
pub async fn has_jev_key(connection: &zbus::Connection) -> zbus::Result<bool> {
    let service = proxy(connection, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service.call("SearchItems", &(attributes(),)).await?;
    Ok(!unlocked.is_empty() || !locked.is_empty())
}

/// Stores Jev's key in the default collection, replacing the one there. The keyring may ask for its own
/// password first; dismissing that comes back as `Ok(false)`.
pub async fn store_jev_key(connection: &zbus::Connection, key: Zeroizing<String>) -> zbus::Result<bool> {
    let service = proxy(connection, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let (_, session): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Value::from(""))).await?;
    let result = store(connection, &service, &session, key).await;
    if let Ok(session) = proxy(connection, session.as_str(), "org.freedesktop.Secret.Session").await {
        let _ = session.call_noreply("Close", &()).await;
    }
    result
}

async fn store(connection: &zbus::Connection, service: &zbus::Proxy<'_>, session: &OwnedObjectPath, key: Zeroizing<String>) -> zbus::Result<bool> {
    let mut collection: OwnedObjectPath = service.call("ReadAlias", &("default",)).await?;
    if collection.as_str() == "/" {
        let properties = HashMap::from([("org.freedesktop.Secret.Collection.Label", Value::from("Login"))]);
        let (made, prompt): (OwnedObjectPath, OwnedObjectPath) = service.call("CreateCollection", &(properties, "default")).await?;
        collection = match prompted(connection, &prompt).await? {
            Some(result) if made.as_str() == "/" => OwnedObjectPath::try_from(result)?,
            Some(_) => made,
            None => return Ok(false),
        };
    }
    let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Unlock", &(vec![collection.clone()],)).await?;
    if prompted(connection, &prompt).await?.is_none() {
        return Ok(false);
    }
    let attributes = Value::from(attributes());
    let properties = HashMap::from([("org.freedesktop.Secret.Item.Label", Value::from("Jev API key")), ("org.freedesktop.Secret.Item.Attributes", attributes)]);
    let value = Zeroizing::new(key.as_bytes().to_vec());
    let secret = (session.clone(), Vec::<u8>::new(), value.as_slice(), "text/plain");
    let collection = proxy(connection, collection.as_str(), "org.freedesktop.Secret.Collection").await?;
    let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = collection.call("CreateItem", &(properties, secret, true)).await?;
    Ok(prompted(connection, &prompt).await?.is_some())
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
