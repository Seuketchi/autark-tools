//! NetworkManager, for distros that use it: the first Wi-Fi device, its access points, and
//! joining through saved connections or new ones.

use std::collections::{HashMap, HashSet};

use async_channel::{Receiver, Sender};
use futures_util::StreamExt;
use zbus::{OwnedObjectPath, OwnedVariant, Variant};

use super::{Command, Network, Update, Wifi, bars_from_strength, changes, never, sorted};

const SERVICE: &str = "org.freedesktop.NetworkManager";
const WIFI_DEVICE: u32 = 2;

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn activate_connection(
        &self,
        connection: &OwnedObjectPath,
        device: &OwnedObjectPath,
        specific_object: &OwnedObjectPath,
    ) -> zbus::Result<OwnedObjectPath>;
    fn add_and_activate_connection(
        &self,
        connection: HashMap<&str, HashMap<&str, Variant<'_>>>,
        device: &OwnedObjectPath,
        specific_object: &OwnedObjectPath,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
    #[zbus(property)]
    fn wireless_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_wireless_enabled(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager/Settings"
)]
trait Settings {
    fn list_connections(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings.Connection",
    default_service = "org.freedesktop.NetworkManager"
)]
trait Connection {
    fn get_settings(&self) -> zbus::Result<HashMap<String, HashMap<String, OwnedVariant>>>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.Device", default_service = "org.freedesktop.NetworkManager")]
trait Device {
    #[zbus(property)]
    fn device_type(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device.Wireless",
    default_service = "org.freedesktop.NetworkManager"
)]
trait Wireless {
    fn get_all_access_points(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn request_scan(&self, options: HashMap<&str, Variant<'_>>) -> zbus::Result<()>;
    #[zbus(property)]
    fn active_access_point(&self) -> zbus::Result<OwnedObjectPath>;
    #[zbus(property)]
    fn last_scan(&self) -> zbus::Result<i64>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.AccessPoint", default_service = "org.freedesktop.NetworkManager")]
trait AccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> zbus::Result<Vec<u8>>;
    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;
    #[zbus(property)]
    fn flags(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn wpa_flags(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn rsn_flags(&self) -> zbus::Result<u32>;
}

struct Backend {
    connection: zbus::Connection,
    manager: NetworkManagerProxy<'static>,
    device: OwnedObjectPath,
    wireless: WirelessProxy<'static>,
}

/// Returns `Ok(false)` when NetworkManager isn't running.
pub async fn run(updates: Sender<Update>, commands: Receiver<Command>) -> zbus::Result<bool> {
    let connection = zbus::Connection::system().await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    if !bus.name_has_owner(SERVICE.try_into()?).await? {
        let _ = updates.send(Update::Status(Wifi::Unavailable)).await;
        return Ok(false);
    }
    let manager = NetworkManagerProxy::new(&connection).await?;
    let mut device = None;
    for path in manager.get_devices().await? {
        let proxy = DeviceProxy::builder(&connection).path(path.clone()).build().await?;
        if proxy.device_type().await? == WIFI_DEVICE {
            device = Some(path);
            break;
        }
    }
    let Some(device) = device else {
        let _ = updates.send(Update::Status(Wifi::Unavailable)).await;
        return Ok(true);
    };
    let wireless = WirelessProxy::builder(&connection).path(device.clone()).build().await?;
    let backend = Backend { connection, manager, device, wireless };

    let mut access_points = changes(backend.wireless.receive_active_access_point_changed().await);
    let mut scans = changes(backend.wireless.receive_last_scan_changed().await);
    let mut enabled = changes(backend.manager.receive_wireless_enabled_changed().await);
    let mut commands = Box::pin(commands).fuse();
    let mut strengths = never();
    let mut watched = None;

    loop {
        let active = backend.wireless.active_access_point().await?;
        let status = if !backend.manager.wireless_enabled().await? {
            Wifi::Off
        } else if active.as_str() == "/" {
            Wifi::Disconnected
        } else {
            let access_point = backend.access_point(active.clone()).await?;
            if watched.as_ref() != Some(&active) {
                strengths = changes(access_point.receive_strength_changed().await);
                watched = Some(active.clone());
            }
            Wifi::Connected {
                name: String::from_utf8_lossy(&access_point.ssid().await?).into_owned(),
                bars: bars_from_strength(access_point.strength().await?),
            }
        };
        if active.as_str() == "/" {
            strengths = never();
            watched = None;
        }
        if updates.send(Update::Status(status)).await.is_err() {
            return Ok(true);
        }
        futures_util::select! {
            _ = access_points.next() => {
                let _ = updates.send(Update::Networks(backend.networks().await?)).await;
            }
            _ = scans.next() => {
                let _ = updates.send(Update::Networks(backend.networks().await?)).await;
            }
            _ = enabled.next() => {}
            _ = strengths.next() => {}
            command = commands.next() => match command {
                Some(command) => backend.handle(command, &updates).await?,
                None => return Ok(true),
            },
        }
    }
}

impl Backend {
    async fn access_point(&self, path: OwnedObjectPath) -> zbus::Result<AccessPointProxy<'static>> {
        AccessPointProxy::builder(&self.connection).path(path).build().await
    }

    async fn handle(&self, command: Command, updates: &Sender<Update>) -> zbus::Result<()> {
        match command {
            Command::Scan => {
                // Refused when the last scan was moments ago; the cached list is still shown.
                let _ = self.wireless.request_scan(HashMap::new()).await;
                let _ = updates.send(Update::Networks(self.networks().await?)).await;
            }
            Command::SetPowered(on) => {
                if let Err(err) = self.manager.set_wireless_enabled(on).await {
                    eprintln!("wifi: turning Wi-Fi {}: {err}", if on { "on" } else { "off" });
                }
            }
            Command::Connect { id, passphrase } => {
                if let Err(err) = self.connect(&id, passphrase).await {
                    eprintln!("wifi: joining {id}: {err}");
                    let _ = updates.send(Update::Failed(id)).await;
                }
            }
        }
        Ok(())
    }

    /// `ssid` is the network's name: a saved connection is reused unless a new password is given.
    async fn connect(&self, ssid: &str, passphrase: Option<String>) -> zbus::Result<()> {
        if passphrase.is_none()
            && let Some(saved) = self.saved_connections().await?.remove(ssid.as_bytes())
        {
            let none = OwnedObjectPath::try_from("/")?;
            self.manager.activate_connection(&saved, &self.device, &none).await?;
            return Ok(());
        }
        let mut strongest = None;
        for path in self.wireless.get_all_access_points().await? {
            let access_point = self.access_point(path.clone()).await?;
            if access_point.ssid().await? == ssid.as_bytes() {
                let strength = access_point.strength().await?;
                if strongest.as_ref().is_none_or(|(_, best)| strength > *best) {
                    strongest = Some((path, strength));
                }
            }
        }
        let Some((access_point, _)) = strongest else {
            return Err(zbus::Error::Failure(format!("{ssid} is out of range")));
        };
        let mut settings = HashMap::new();
        if let Some(passphrase) = passphrase {
            settings.insert(
                "802-11-wireless-security",
                HashMap::from([("key-mgmt", Variant::from("wpa-psk")), ("psk", Variant::from(passphrase))]),
            );
        }
        self.manager.add_and_activate_connection(settings, &self.device, &access_point).await?;
        Ok(())
    }

    /// Saved Wi-Fi connections by SSID.
    async fn saved_connections(&self) -> zbus::Result<HashMap<Vec<u8>, OwnedObjectPath>> {
        let mut saved = HashMap::new();
        for path in SettingsProxy::new(&self.connection).await?.list_connections().await? {
            let proxy = ConnectionProxy::builder(&self.connection).path(path.clone()).build().await?;
            let Ok(settings) = proxy.get_settings().await else {
                continue;
            };
            let ssid = settings
                .get("802-11-wireless")
                .and_then(|wireless| wireless.get("ssid"))
                .and_then(|ssid| Vec::<u8>::try_from(ssid.try_clone().ok()?).ok());
            if let Some(ssid) = ssid {
                saved.insert(ssid, path);
            }
        }
        Ok(saved)
    }

    async fn networks(&self) -> zbus::Result<Vec<Network>> {
        let saved: HashSet<Vec<u8>> = self.saved_connections().await?.into_keys().collect();
        let active = self.wireless.active_access_point().await?;
        let mut networks = Vec::new();
        for path in self.wireless.get_all_access_points().await? {
            let access_point = self.access_point(path.clone()).await?;
            let ssid = access_point.ssid().await?;
            if ssid.is_empty() {
                continue;
            }
            let name = String::from_utf8_lossy(&ssid).into_owned();
            networks.push(Network {
                id: name.clone(),
                name,
                bars: bars_from_strength(access_point.strength().await?),
                secured: access_point.flags().await? & 1 != 0
                    || access_point.wpa_flags().await? != 0
                    || access_point.rsn_flags().await? != 0,
                known: saved.contains(&ssid),
                connected: path == active,
            });
        }
        Ok(sorted(networks))
    }
}
