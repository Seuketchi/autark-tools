//! The laptop's lid, from UPower on the system bus (logind doesn't announce changes to `LidClosed`). Without
//! UPower or a lid, it counts as open.

use futures_util::StreamExt;

#[zbus::proxy(interface = "org.freedesktop.UPower", default_service = "org.freedesktop.UPower", default_path = "/org/freedesktop/UPower")]
trait UPower {
    #[zbus(property)]
    fn lid_is_closed(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn lid_is_present(&self) -> zbus::Result<bool>;
}

/// Sends whether the lid is shut, now and whenever it changes. Ends quietly without UPower.
pub async fn watch(changes: async_channel::Sender<bool>) {
    let Ok(connection) = zbus::Connection::system().await else { return };
    let Ok(upower) = UPowerProxy::new(&connection).await else { return };
    if !upower.lid_is_present().await.unwrap_or(false) {
        return;
    }
    let mut stream = upower.receive_lid_is_closed_changed().await;
    if let Ok(closed) = upower.lid_is_closed().await {
        let _ = changes.send(closed).await;
    }
    while let Some(change) = stream.next().await {
        if let Ok(closed) = change.get().await
            && changes.send(closed).await.is_err()
        {
            return;
        }
    }
}
