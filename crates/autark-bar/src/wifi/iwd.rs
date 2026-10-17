//! iwd: the station's state and network name, and its signal level through a signal-level agent,
//! so nothing is polled.

use async_channel::Sender;
use futures_util::StreamExt;
use zbus::{ObjectPath, OwnedObjectPath};

use super::Wifi;

const SERVICE: &str = "net.connman.iwd";
const AGENT_PATH: &str = "/org/autark/Bar/SignalLevel";
/// In 100 × dBm: iwd reports level 0 at or above -60 dBm, 1 at or above -70, and 2 below.
const LEVELS: [i16; 2] = [-6000, -7000];

#[zbus::proxy(interface = "net.connman.iwd.Station", default_service = "net.connman.iwd")]
trait Station {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn connected_network(&self) -> zbus::Result<OwnedObjectPath>;
    fn register_signal_level_agent(&self, path: &ObjectPath<'_>, levels: &[i16]) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "net.connman.iwd.Network", default_service = "net.connman.iwd")]
trait Network {
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
}

struct Agent {
    levels: Sender<u8>,
}

#[zbus::interface(name = "net.connman.iwd.SignalLevelAgent")]
impl Agent {
    async fn changed(&self, _station: ObjectPath<'_>, level: u8) {
        let _ = self.levels.send(level).await;
    }

    fn release(&self, _station: ObjectPath<'_>) {}
}

/// Returns `Ok(false)` when iwd isn't running, so the caller can try NetworkManager.
pub async fn watch(updates: Sender<Wifi>) -> zbus::Result<bool> {
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
    let station = objects
        .get_managed_objects()
        .await?
        .into_iter()
        .find(|(_, interfaces)| interfaces.keys().any(|name| name.as_str() == "net.connman.iwd.Station"))
        .map(|(path, _)| path);
    let Some(station) = station else {
        let _ = updates.send(Wifi::Unavailable).await;
        return Ok(true);
    };
    let station = StationProxy::builder(&connection).path(station).build().await?;

    let (levels, received) = async_channel::unbounded();
    connection.object_server().at(AGENT_PATH, Agent { levels }).await?;
    station
        .register_signal_level_agent(&ObjectPath::try_from(AGENT_PATH)?, &LEVELS)
        .await?;

    let mut states = station.receive_state_changed().await.fuse();
    let mut received = Box::pin(received).fuse();
    let mut level = 1;
    loop {
        let wifi = match station.state().await?.as_str() {
            "connected" | "roaming" => {
                let network = NetworkProxy::builder(&connection)
                    .path(station.connected_network().await?)
                    .build()
                    .await?;
                Wifi::Connected { name: network.name().await?, bars: 3 - level.min(2) }
            }
            _ => Wifi::Disconnected,
        };
        if updates.send(wifi).await.is_err() {
            break;
        }
        futures_util::select! {
            change = states.next() => if change.is_none() { break },
            new = received.next() => match new {
                Some(new) => level = new,
                None => break,
            },
        }
    }
    Ok(true)
}
