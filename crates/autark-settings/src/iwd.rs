//! iwd, Autark's Wi-Fi daemon, read whole: one `GetManagedObjects` gives the device, its station, the
//! networks in range and the known ones, and the station orders the networks by signal. Settings answers
//! iwd's passphrase requests itself; iwd asks the agent of whoever called `Connect`, so the bar's own agent
//! doesn't get in the way.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use zbus::{OwnedObjectPath, OwnedVariant};
use zeroize::Zeroizing;

pub const SERVICE: &str = "net.connman.iwd";
pub const DAEMON: &str = "iwd";
const AGENT: &str = "/org/autark/Settings/Passphrase";

#[zbus::proxy(interface = "net.connman.iwd.Device", default_service = "net.connman.iwd")]
pub trait Device {
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "net.connman.iwd.Station", default_service = "net.connman.iwd")]
pub trait Station {
    fn scan(&self) -> zbus::Result<()>;
    fn disconnect(&self) -> zbus::Result<()>;
    fn connect_hidden_network(&self, name: &str) -> zbus::Result<()>;
    fn get_ordered_networks(&self) -> zbus::Result<Vec<(OwnedObjectPath, i16)>>;
}

#[zbus::proxy(interface = "net.connman.iwd.StationDiagnostic", default_service = "net.connman.iwd")]
pub trait StationDiagnostic {
    fn get_diagnostics(&self) -> zbus::Result<HashMap<String, OwnedVariant>>;
}

#[zbus::proxy(interface = "net.connman.iwd.Network", default_service = "net.connman.iwd")]
pub trait Network {
    fn connect(&self) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "net.connman.iwd.KnownNetwork", default_service = "net.connman.iwd")]
pub trait KnownNetwork {
    fn forget(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn set_auto_connect(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "net.connman.iwd.AgentManager", default_service = "net.connman.iwd", default_path = "/net/connman/iwd")]
trait AgentManager {
    fn register_agent(&self, path: &zbus::ObjectPath<'_>) -> zbus::Result<()>;
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// None when there's no Wi-Fi device.
    pub device: Option<WifiDevice>,
    /// In range, strongest first.
    pub networks: Vec<InRange>,
    /// Joined before, most recent first.
    pub known: Vec<Known>,
}

#[derive(Clone, Debug)]
pub struct WifiDevice {
    pub path: OwnedObjectPath,
    /// The interface, such as wlan0.
    pub interface: String,
    pub powered: bool,
    /// The station exists while the device is powered and in station mode.
    pub state: Option<String>,
    pub connected: Option<OwnedObjectPath>,
    pub scanning: bool,
}

#[derive(Clone, Debug)]
pub struct InRange {
    pub path: OwnedObjectPath,
    pub name: String,
    /// "open", "psk", "8021x" or "wep".
    pub kind: String,
    pub connected: bool,
    pub known: Option<OwnedObjectPath>,
    /// In 100 × dBm.
    pub signal: i16,
}

#[derive(Clone, Debug)]
pub struct Known {
    pub path: OwnedObjectPath,
    pub name: String,
    pub kind: String,
    pub auto_connect: bool,
    last_connected: String,
}

type Properties = HashMap<String, OwnedVariant>;

fn text(properties: &Properties, key: &str) -> String {
    properties.get(key).and_then(|value| String::try_from(value.try_clone().ok()?).ok()).unwrap_or_default()
}

fn flag(properties: &Properties, key: &str) -> bool {
    properties.get(key).and_then(|value| bool::try_from(value).ok()).unwrap_or(false)
}

fn path(properties: &Properties, key: &str) -> Option<OwnedObjectPath> {
    properties.get(key).and_then(|value| OwnedObjectPath::try_from(value.try_clone().ok()?).ok())
}

pub async fn read(connection: &zbus::Connection) -> zbus::Result<Snapshot> {
    let objects = zbus::fdo::ObjectManagerProxy::builder(connection).destination(SERVICE).path("/").build().await?.get_managed_objects().await?;
    let interface = |interfaces: &std::collections::BTreeMap<zbus::names::OwnedInterfaceName, Properties>, name: &str| {
        interfaces.iter().find(|(key, _)| key.as_str() == name).map(|(_, properties)| properties.clone())
    };
    let mut snapshot = Snapshot::default();
    let mut networks = Vec::new();
    for (object, interfaces) in &objects {
        if let Some(device) = interface(interfaces, "net.connman.iwd.Device")
            && snapshot.device.is_none()
        {
            let station = interface(interfaces, "net.connman.iwd.Station");
            snapshot.device = Some(WifiDevice {
                path: object.clone(),
                interface: text(&device, "Name"),
                powered: flag(&device, "Powered"),
                state: station.as_ref().map(|station| text(station, "State")),
                connected: station.as_ref().and_then(|station| path(station, "ConnectedNetwork")),
                scanning: station.as_ref().is_some_and(|station| flag(station, "Scanning")),
            });
        }
        if let Some(network) = interface(interfaces, "net.connman.iwd.Network") {
            networks.push(InRange {
                path: object.clone(),
                name: text(&network, "Name"),
                kind: text(&network, "Type"),
                connected: flag(&network, "Connected"),
                known: path(&network, "KnownNetwork"),
                signal: i16::MIN,
            });
        }
        if let Some(known) = interface(interfaces, "net.connman.iwd.KnownNetwork") {
            snapshot.known.push(Known {
                path: object.clone(),
                name: text(&known, "Name"),
                kind: text(&known, "Type"),
                auto_connect: flag(&known, "AutoConnect"),
                last_connected: text(&known, "LastConnectedTime"),
            });
        }
    }
    if let Some(device) = &snapshot.device
        && device.state.is_some()
    {
        let station = StationProxy::builder(connection).path(device.path.clone()).build().await?;
        // Networks the station doesn't order (being dropped) go last.
        if let Ok(ordered) = station.get_ordered_networks().await {
            for (path, signal) in ordered {
                if let Some(network) = networks.iter_mut().find(|network| network.path == path) {
                    network.signal = signal;
                }
            }
        }
    }
    networks.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));
    snapshot.networks = networks;
    // ISO 8601 times sort as text.
    snapshot.known.sort_by(|a, b| b.last_connected.cmp(&a.last_connected).then_with(|| a.name.cmp(&b.name)));
    Ok(snapshot)
}

/// Calls `changed` whenever anything of iwd's changes (a property, an object coming or going, or iwd itself
/// starting or stopping).
pub async fn watch(connection: &zbus::Connection, changed: impl Fn()) -> zbus::Result<()> {
    let owners = zbus::fdo::DBusProxy::new(connection).await?.receive_name_owner_changed_with_args(&[(0, SERVICE)]).await?.map(|_| ());
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).path_namespace("/net/connman/iwd").build()?;
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

/// The security of the network the station is connected to, as iwd's diagnostics name it.
pub async fn security(connection: &zbus::Connection, device: &OwnedObjectPath) -> Option<String> {
    let diagnostics = StationDiagnosticProxy::builder(connection).path(device.clone()).build().await.ok()?.get_diagnostics().await.ok()?;
    let security = text(&diagnostics, "Security");
    (!security.is_empty()).then_some(security)
}

/// Where the next passphrase iwd asks for is put.
pub type Passphrase = Arc<Mutex<Option<Zeroizing<String>>>>;

/// Answers iwd's passphrase request with the one typed in Settings just before joining.
struct Agent {
    passphrase: Passphrase,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl Agent {
    fn request_passphrase(&self, _network: zbus::ObjectPath<'_>) -> zbus::fdo::Result<String> {
        let passphrase = self.passphrase.lock().ok().and_then(|mut passphrase| passphrase.take());
        passphrase.map(|passphrase| passphrase.as_str().to_owned()).ok_or_else(|| zbus::fdo::Error::Failed("No password was given".into()))
    }

    fn cancel(&self, _reason: String) {}

    fn release(&self) {}
}

/// Registers Settings' agent; what's put in the returned slot answers the next passphrase request.
pub async fn agent(connection: &zbus::Connection) -> zbus::Result<Passphrase> {
    let passphrase = Arc::new(Mutex::new(None));
    // Again after iwd restarts: the object stays served, and a fresh one replaces it.
    connection.object_server().remove::<Agent, _>(AGENT).await.ok();
    connection.object_server().at(AGENT, Agent { passphrase: passphrase.clone() }).await?;
    AgentManagerProxy::new(connection).await?.register_agent(&zbus::ObjectPath::try_from(AGENT)?).await?;
    Ok(passphrase)
}

/// The interface's IPv4 address, if it has one.
pub fn address(interface: &str) -> Option<String> {
    let mut found = None;
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, which is walked read-only and freed once.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return None;
        }
        let mut entry = list;
        while !entry.is_null() {
            let item = &*entry;
            if !item.ifa_addr.is_null() && i32::from((*item.ifa_addr).sa_family) == libc::AF_INET && std::ffi::CStr::from_ptr(item.ifa_name).to_bytes() == interface.as_bytes() {
                let address = &*(item.ifa_addr as *const libc::sockaddr_in);
                found = Some(std::net::Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)).to_string());
                break;
            }
            entry = item.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

/// Signal as the 0–3 bars of the Wi-Fi fan.
pub fn bars(signal: i16) -> u8 {
    match signal {
        -6000.. => 3,
        -7000.. => 2,
        -8000.. => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_reads_as_bars() {
        assert_eq!(bars(-5500), 3);
        assert_eq!(bars(-6500), 2);
        assert_eq!(bars(-7900), 1);
        assert_eq!(bars(-9000), 0);
        assert_eq!(bars(i16::MIN), 0);
    }

    #[test]
    fn loopback_has_an_address() {
        assert_eq!(address("lo").as_deref(), Some("127.0.0.1"));
        assert_eq!(address("no-such-interface"), None);
    }
}
