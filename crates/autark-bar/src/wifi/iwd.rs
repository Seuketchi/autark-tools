//! iwd: the device's power, the station's state and networks, joining with a passphrase agent,
//! and signal strength through a signal-level agent, so nothing is polled.

use std::sync::{Arc, Mutex};

use async_channel::{Receiver, Sender};
use futures_util::StreamExt;
use zbus::{ObjectPath, OwnedObjectPath};

use super::{Command, Network, Update, Wifi, bars_from_dbm, changes, never, sorted};

const SERVICE: &str = "net.connman.iwd";
const SIGNAL_AGENT: &str = "/org/autark/Bar/SignalLevel";
const PASSPHRASE_AGENT: &str = "/org/autark/Bar/Passphrase";
/// In 100 × dBm: iwd reports level 0 at or above -60 dBm, 1 at or above -70, and 2 below.
const LEVELS: [i16; 2] = [-6000, -7000];

#[zbus::proxy(interface = "net.connman.iwd.Device", default_service = "net.connman.iwd")]
trait Device {
    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "net.connman.iwd.Station", default_service = "net.connman.iwd")]
trait Station {
    fn scan(&self) -> zbus::Result<()>;
    fn get_ordered_networks(&self) -> zbus::Result<Vec<(OwnedObjectPath, i16)>>;
    fn register_signal_level_agent(&self, path: &ObjectPath<'_>, levels: &[i16]) -> zbus::Result<()>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn scanning(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn connected_network(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "net.connman.iwd.Network", default_service = "net.connman.iwd")]
trait Network {
    fn connect(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property, name = "Type")]
    fn kind(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn connected(&self) -> zbus::Result<bool>;
    /// Only present for networks joined before.
    #[zbus(property)]
    fn known_network(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "net.connman.iwd.AgentManager",
    default_service = "net.connman.iwd",
    default_path = "/net/connman/iwd"
)]
trait AgentManager {
    fn register_agent(&self, path: &ObjectPath<'_>) -> zbus::Result<()>;
}

struct SignalAgent {
    levels: Sender<u8>,
}

#[zbus::interface(name = "net.connman.iwd.SignalLevelAgent")]
impl SignalAgent {
    async fn changed(&self, _station: ObjectPath<'_>, level: u8) {
        let _ = self.levels.send(level).await;
    }

    fn release(&self, _station: ObjectPath<'_>) {}
}

/// Answers iwd's passphrase request with the one typed into the panel just before joining.
struct PassphraseAgent {
    passphrase: Arc<Mutex<Option<String>>>,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl PassphraseAgent {
    fn request_passphrase(&self, _network: ObjectPath<'_>) -> zbus::fdo::Result<String> {
        self.passphrase
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| zbus::fdo::Error::Failed("no passphrase given".to_owned()))
    }

    fn cancel(&self, _reason: String) {}

    fn release(&self) {}
}

/// Returns `Ok(false)` when iwd isn't running, so the caller can try NetworkManager.
pub async fn run(updates: Sender<Update>, commands: Receiver<Command>) -> zbus::Result<bool> {
    let connection = zbus::Connection::system().await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    if !bus.name_has_owner(SERVICE.try_into()?).await? {
        return Ok(false);
    }
    let objects = zbus::fdo::ObjectManagerProxy::builder(&connection)
        .destination(SERVICE)
        .path("/")
        .build()
        .await?;
    // The station lives on the device's object while the device is powered.
    let device = objects
        .get_managed_objects()
        .await?
        .into_iter()
        .find(|(_, interfaces)| interfaces.keys().any(|name| name.as_str() == "net.connman.iwd.Device"))
        .map(|(path, _)| path);
    let Some(path) = device else {
        let _ = updates.send(Update::Status(Wifi::Unavailable)).await;
        return Ok(true);
    };
    let device = DeviceProxy::builder(&connection).path(path.clone()).build().await?;

    let (levels, received_levels) = async_channel::unbounded();
    connection.object_server().at(SIGNAL_AGENT, SignalAgent { levels }).await?;
    let passphrase = Arc::new(Mutex::new(None));
    connection
        .object_server()
        .at(PASSPHRASE_AGENT, PassphraseAgent { passphrase: passphrase.clone() })
        .await?;
    AgentManagerProxy::new(&connection)
        .await?
        .register_agent(&ObjectPath::try_from(PASSPHRASE_AGENT)?)
        .await?;

    let mut powered = changes(device.receive_powered_changed().await);
    let mut added = changes(objects.receive_interfaces_added().await?);
    let mut removed = changes(objects.receive_interfaces_removed().await?);
    let mut received_levels = Box::pin(received_levels).fuse();
    let mut commands = Box::pin(commands).fuse();
    let mut station: Option<StationProxy<'static>> = None;
    let (mut states, mut scans) = (never(), never());
    let mut level = 1;

    loop {
        let on = device.powered().await?;
        if !on {
            station = None;
        } else if station.is_none() {
            let proxy = StationProxy::builder(&connection).path(path.clone()).build().await?;
            if proxy.state().await.is_ok() {
                states = changes(proxy.receive_state_changed().await);
                scans = changes(proxy.receive_scanning_changed().await);
                let _ = proxy.register_signal_level_agent(&ObjectPath::try_from(SIGNAL_AGENT)?, &LEVELS).await;
                station = Some(proxy);
            }
        }
        if station.is_none() {
            (states, scans) = (never(), never());
        }
        let status = match &station {
            _ if !on => Wifi::Off,
            None => Wifi::Disconnected,
            Some(station) => match station.state().await?.as_str() {
                "connected" | "roaming" => {
                    let network = network(&connection, station.connected_network().await?).await?;
                    Wifi::Connected { name: network.name().await?, bars: 3 - level.min(2) }
                }
                _ => Wifi::Disconnected,
            },
        };
        if updates.send(Update::Status(status)).await.is_err() {
            return Ok(true);
        }
        futures_util::select! {
            _ = powered.next() => {}
            _ = added.next() => {}
            _ = removed.next() => station = None,
            _ = states.next() => {
                if let Some(station) = &station {
                    let _ = updates.send(Update::Networks(networks(&connection, station).await?)).await;
                }
            }
            _ = scans.next() => {
                if let Some(station) = &station
                    && !station.scanning().await.unwrap_or(true)
                {
                    let _ = updates.send(Update::Networks(networks(&connection, station).await?)).await;
                }
            }
            new = received_levels.next() => level = new.unwrap_or(level),
            command = commands.next() => {
                let Some(command) = command else {
                    return Ok(true);
                };
                match command {
                    Command::SetPowered(on) => {
                        if let Err(err) = device.set_powered(on).await {
                            eprintln!("wifi: turning Wi-Fi {}: {err}", if on { "on" } else { "off" });
                        }
                    }
                    Command::Scan => {
                        if let Some(station) = &station {
                            // Refused while a scan is running; the list follows when it ends.
                            let _ = station.scan().await;
                            let _ = updates.send(Update::Networks(networks(&connection, station).await?)).await;
                        }
                    }
                    Command::Connect { id, passphrase: given } => {
                        *passphrase.lock().unwrap() = given;
                        let proxy = network(&connection, OwnedObjectPath::try_from(id.as_str())?).await?;
                        if let Err(err) = proxy.connect().await {
                            let name = proxy.name().await.unwrap_or(id);
                            eprintln!("wifi: joining {name}: {err}");
                            let _ = updates.send(Update::Failed(name)).await;
                        }
                        passphrase.lock().unwrap().take();
                    }
                }
            }
        }
    }
}

async fn network(connection: &zbus::Connection, path: OwnedObjectPath) -> zbus::Result<NetworkProxy<'static>> {
    NetworkProxy::builder(connection).path(path).build().await
}

async fn networks(connection: &zbus::Connection, station: &StationProxy<'_>) -> zbus::Result<Vec<Network>> {
    let mut found = Vec::new();
    for (path, signal) in station.get_ordered_networks().await? {
        let proxy = network(connection, path.clone()).await?;
        found.push(Network {
            id: path.to_string(),
            name: proxy.name().await?,
            bars: bars_from_dbm(signal),
            secured: proxy.kind().await? != "open",
            known: proxy.known_network().await.is_ok(),
            connected: proxy.connected().await?,
        });
    }
    Ok(sorted(found))
}
