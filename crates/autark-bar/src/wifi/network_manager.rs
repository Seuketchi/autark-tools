//! NetworkManager, for distros that use it: the first Wi-Fi device's access point and its strength.

use async_channel::Sender;
use futures_util::StreamExt;
use zbus::OwnedObjectPath;

use super::Wifi;

const SERVICE: &str = "org.freedesktop.NetworkManager";
const WIFI_DEVICE: u32 = 2;

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
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
    #[zbus(property)]
    fn active_access_point(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.AccessPoint", default_service = "org.freedesktop.NetworkManager")]
trait AccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> zbus::Result<Vec<u8>>;
    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;
}

pub async fn watch(updates: Sender<Wifi>) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    if !bus.name_has_owner(SERVICE.try_into()?).await? {
        let _ = updates.send(Wifi::Unavailable).await;
        return Ok(());
    }
    let mut device = None;
    for path in NetworkManagerProxy::new(&connection).await?.get_devices().await? {
        let proxy = DeviceProxy::builder(&connection).path(path.clone()).build().await?;
        if proxy.device_type().await? == WIFI_DEVICE {
            device = Some(path);
            break;
        }
    }
    let Some(device) = device else {
        let _ = updates.send(Wifi::Unavailable).await;
        return Ok(());
    };
    let wireless = WirelessProxy::builder(&connection).path(device).build().await?;
    let mut access_points = wireless.receive_active_access_point_changed().await.fuse();

    loop {
        let path = wireless.active_access_point().await?;
        if path.as_str() == "/" {
            if updates.send(Wifi::Disconnected).await.is_err() || access_points.next().await.is_none() {
                return Ok(());
            }
            continue;
        }
        let access_point = AccessPointProxy::builder(&connection).path(path).build().await?;
        let name = String::from_utf8_lossy(&access_point.ssid().await?).into_owned();
        let mut strength = access_point.strength().await?;
        let mut strengths = access_point.receive_strength_changed().await.fuse();
        loop {
            let wifi = Wifi::Connected { name: name.clone(), bars: bars(strength) };
            if updates.send(wifi).await.is_err() {
                return Ok(());
            }
            futures_util::select! {
                change = strengths.next() => match change {
                    Some(change) => strength = change.get().await?,
                    None => break,
                },
                change = access_points.next() => match change {
                    Some(_) => break,
                    None => return Ok(()),
                },
            }
        }
    }
}

/// NetworkManager's 0–100 strength as one to three arcs.
fn bars(strength: u8) -> u8 {
    match strength {
        0..=33 => 1,
        34..=66 => 2,
        _ => 3,
    }
}
