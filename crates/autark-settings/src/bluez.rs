//! BlueZ read whole: one `GetManagedObjects` gives the adapter, the paired devices and the ones nearby.
//! Pairing goes through Settings' own agent, whose questions (confirm a passkey, show a PIN) come to the
//! pane as `Ask`s; BlueZ asks the agent of whoever called `Pair`, so no default agent is taken.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use zbus::{OwnedObjectPath, OwnedVariant};

pub const SERVICE: &str = "org.bluez";
pub const DAEMON: &str = "BlueZ (bluetoothd)";
const AGENT: &str = "/org/autark/Settings/Pairing";

#[zbus::proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez")]
pub trait Adapter {
    fn start_discovery(&self) -> zbus::Result<()>;
    fn stop_discovery(&self) -> zbus::Result<()>;
    fn remove_device(&self, device: &zbus::ObjectPath<'_>) -> zbus::Result<()>;
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn set_discoverable(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
pub trait Device {
    fn connect(&self) -> zbus::Result<()>;
    fn disconnect(&self) -> zbus::Result<()>;
    fn pair(&self) -> zbus::Result<()>;
    fn cancel_pairing(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn set_trusted(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.AgentManager1", default_service = "org.bluez", default_path = "/org/bluez")]
trait AgentManager {
    fn register_agent(&self, agent: &zbus::ObjectPath<'_>, capability: &str) -> zbus::Result<()>;
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub adapter: Option<Radio>,
    /// Paired, by name.
    pub mine: Vec<Gadget>,
    /// Not paired, with a name and in range, by name.
    pub nearby: Vec<Gadget>,
}

#[derive(Clone, Debug)]
pub struct Radio {
    pub path: OwnedObjectPath,
    /// What other devices see this laptop called.
    pub alias: String,
    pub powered: bool,
    pub discovering: bool,
}

#[derive(Clone, Debug)]
pub struct Gadget {
    pub path: OwnedObjectPath,
    pub name: String,
    /// The freedesktop icon name BlueZ gives from the device class, such as audio-headphones.
    pub icon: String,
    pub connected: bool,
    pub battery: Option<u8>,
}

type Properties = std::collections::HashMap<String, OwnedVariant>;

fn text(properties: &Properties, key: &str) -> String {
    properties.get(key).and_then(|value| String::try_from(value.try_clone().ok()?).ok()).unwrap_or_default()
}

fn flag(properties: &Properties, key: &str) -> bool {
    properties.get(key).and_then(|value| bool::try_from(value).ok()).unwrap_or(false)
}

pub async fn read(connection: &zbus::Connection) -> zbus::Result<Snapshot> {
    let objects = zbus::fdo::ObjectManagerProxy::builder(connection).destination(SERVICE).path("/").build().await?.get_managed_objects().await?;
    let interface = |interfaces: &BTreeMap<zbus::names::OwnedInterfaceName, Properties>, name: &str| interfaces.iter().find(|(key, _)| key.as_str() == name).map(|(_, properties)| properties.clone());
    let mut snapshot = Snapshot::default();
    for (object, interfaces) in &objects {
        if let Some(adapter) = interface(interfaces, "org.bluez.Adapter1")
            && snapshot.adapter.is_none()
        {
            snapshot.adapter = Some(Radio { path: object.clone(), alias: text(&adapter, "Alias"), powered: flag(&adapter, "Powered"), discovering: flag(&adapter, "Discovering") });
        }
        let Some(device) = interface(interfaces, "org.bluez.Device1") else { continue };
        let address = text(&device, "Address");
        let alias = text(&device, "Alias");
        // A device that hasn't told its name gets its address, with dashes, as its alias.
        let named = !alias.is_empty() && alias.replace('-', ":") != address;
        let gadget = Gadget {
            path: object.clone(),
            name: if named { alias } else { address },
            icon: text(&device, "Icon"),
            connected: flag(&device, "Connected"),
            battery: interface(interfaces, "org.bluez.Battery1").and_then(|battery| battery.get("Percentage").and_then(|value| u8::try_from(value).ok())),
        };
        if flag(&device, "Paired") {
            snapshot.mine.push(gadget);
        } else if named && device.contains_key("RSSI") {
            snapshot.nearby.push(gadget);
        }
    }
    // By name alone, so a row stays under the pointer as its device connects or its signal changes.
    snapshot.mine.sort_by_key(|gadget| gadget.name.to_lowercase());
    snapshot.nearby.sort_by_key(|gadget| gadget.name.to_lowercase());
    Ok(snapshot)
}

/// Calls `changed` whenever anything of BlueZ's changes, BlueZ starting or stopping included.
pub async fn watch(connection: &zbus::Connection, changed: impl Fn()) -> zbus::Result<()> {
    let owners = zbus::fdo::DBusProxy::new(connection).await?.receive_name_owner_changed_with_args(&[(0, SERVICE)]).await?.map(|_| ());
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).path_namespace("/org/bluez").build()?;
    let properties = zbus::MessageStream::for_match_rule(rule, connection, std::num::NonZeroUsize::new(64)).await?;
    let objects = zbus::fdo::ObjectManagerProxy::builder(connection).destination(SERVICE).path("/").build().await?;
    let added = objects.receive_interfaces_added().await?.map(|_| ());
    let removed = objects.receive_interfaces_removed().await?.map(|_| ());
    let mut all = futures_util::stream::select(futures_util::stream::select(properties.map(|_| ()), added), futures_util::stream::select(removed, owners));
    while all.next().await.is_some() {
        changed();
    }
    Ok(())
}

/// What the agent needs from the person pairing.
pub enum Ask {
    /// Both sides show this passkey; `answer` says whether they match.
    Confirm { device: String, passkey: u32, answer: async_channel::Sender<bool> },
    /// Type this on the device.
    Show { device: String, code: String },
    /// Pairing ended or was cancelled from BlueZ's side.
    Done,
}

struct Agent {
    asks: async_channel::Sender<Ask>,
}

fn rejected() -> zbus::fdo::Error {
    zbus::fdo::Error::Failed("org.bluez.Error.Rejected".into())
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn request_confirmation(&self, device: zbus::ObjectPath<'_>, passkey: u32) -> zbus::fdo::Result<()> {
        let (answer, answered) = async_channel::bounded(1);
        self.asks.send(Ask::Confirm { device: device.to_string(), passkey, answer }).await.map_err(|_| rejected())?;
        if answered.recv().await.unwrap_or(false) { Ok(()) } else { Err(rejected()) }
    }

    /// Older devices that can type but not show: a code is made here and typed there.
    async fn request_pin_code(&self, device: zbus::ObjectPath<'_>) -> zbus::fdo::Result<String> {
        let code = format!("{:06}", random() % 1_000_000);
        self.asks.send(Ask::Show { device: device.to_string(), code: code.clone() }).await.map_err(|_| rejected())?;
        Ok(code)
    }

    async fn request_passkey(&self, device: zbus::ObjectPath<'_>) -> zbus::fdo::Result<u32> {
        let passkey = random() % 1_000_000;
        self.asks.send(Ask::Show { device: device.to_string(), code: format!("{passkey:06}") }).await.map_err(|_| rejected())?;
        Ok(passkey)
    }

    async fn display_passkey(&self, device: zbus::ObjectPath<'_>, passkey: u32, _entered: u16) {
        let _ = self.asks.send(Ask::Show { device: device.to_string(), code: format!("{passkey:06}") }).await;
    }

    async fn display_pin_code(&self, device: zbus::ObjectPath<'_>, code: String) {
        let _ = self.asks.send(Ask::Show { device: device.to_string(), code }).await;
    }

    /// Pairing that needs no code ("just works") is asked for only after Pair was pressed here.
    fn request_authorization(&self, _device: zbus::ObjectPath<'_>) {}

    /// A paired device's services (audio, input) are let through.
    fn authorize_service(&self, _device: zbus::ObjectPath<'_>, _uuid: String) {}

    async fn cancel(&self) {
        let _ = self.asks.send(Ask::Done).await;
    }

    fn release(&self) {}
}

/// Registers Settings' pairing agent; its questions arrive on the returned channel.
pub async fn agent(connection: &zbus::Connection) -> zbus::Result<async_channel::Receiver<Ask>> {
    let (asks, received) = async_channel::unbounded();
    connection.object_server().remove::<Agent, _>(AGENT).await.ok();
    connection.object_server().at(AGENT, Agent { asks }).await?;
    AgentManagerProxy::new(connection).await?.register_agent(&zbus::ObjectPath::try_from(AGENT)?, "KeyboardDisplay").await?;
    Ok(received)
}

/// Enough randomness for a pairing code, from the kernel.
fn random() -> u32 {
    let mut bytes = [0; 4];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes));
    u32::from_ne_bytes(bytes)
}

/// A passkey as the design shows it: "482 913".
pub fn spaced(code: &str) -> String {
    if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) { format!("{} {}", &code[..3], &code[3..]) } else { code.to_owned() }
}

/// A device's glyph, by BlueZ's icon name.
pub fn glyph(icon: &str) -> &'static str {
    match icon {
        "audio-headphones" | "audio-headset" | "audio-card" => "M4 15v-3a8 8 0 0 1 16 0v3M4 15h3v5H4zM17 15h3v5h-3z",
        "audio-speakers" | "multimedia-player" => "M7 3h10v18H7zM12 14a2.5 2.5 0 1 0 0 .01M12 7h.01",
        "input-mouse" | "input-tablet" => "M12 3a6 6 0 0 1 6 6v6a6 6 0 0 1-12 0V9a6 6 0 0 1 6-6zM12 7v3",
        "input-keyboard" => "M3 6h18v12H3zM7 10h.01M11 10h.01M15 10h.01M7 14h10",
        "input-gaming" => "M6 9h12a4 4 0 0 1 0 8H6a4 4 0 0 1 0-8zM8 11v4M6 13h4M16 12h.01M18 14h.01",
        "phone" => "M8 3h8v18H8zM11 18h2",
        "computer" => "M4 5h16v11H4zM2 19h20",
        _ => "M7 7l10 10l-5 4V3l5 4L7 17",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passkeys_read_in_threes() {
        assert_eq!(spaced("482913"), "482 913");
        assert_eq!(spaced("0000"), "0000");
    }
}
