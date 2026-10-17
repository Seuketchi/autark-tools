//! Fingerprint unlock through fprintd, alongside the password: the sensor is watched for as long
//! as the screen is locked, if the user has fingers enrolled.

use futures_util::StreamExt;
use zbus::OwnedObjectPath;

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Manager",
    default_service = "net.reactivated.Fprint",
    default_path = "/net/reactivated/Fprint/Manager"
)]
trait Manager {
    fn get_default_device(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "net.reactivated.Fprint.Device", default_service = "net.reactivated.Fprint")]
trait Device {
    fn list_enrolled_fingers(&self, username: &str) -> zbus::Result<Vec<String>>;
    fn claim(&self, username: &str) -> zbus::Result<()>;
    fn release(&self) -> zbus::Result<()>;
    fn verify_start(&self, finger_name: &str) -> zbus::Result<()>;
    fn verify_stop(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn verify_status(&self, result: &str, done: bool) -> zbus::Result<()>;
}

pub enum Event {
    /// The sensor is waiting for a finger.
    Ready,
    Matched,
    NotRecognised,
    /// Too many failed scans; only the password works now.
    GaveUp,
}

/// fprintd itself limits retries poorly, so the lock screen stops after this many misses.
const ATTEMPTS: u32 = 5;

/// Watches the sensor until a finger matches or it gives up. Returns quietly when there's no
/// sensor or no enrolled finger.
pub async fn watch(user: &str, on_event: impl Fn(Event)) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let Ok(path) = ManagerProxy::new(&connection).await?.get_default_device().await else {
        return Ok(());
    };
    let device = DeviceProxy::builder(&connection).path(path).build().await?;
    if device.list_enrolled_fingers(user).await.map_or(true, |fingers| fingers.is_empty()) {
        return Ok(());
    }
    device.claim(user).await?;
    let mut statuses = device.receive_verify_status().await?;
    device.verify_start("any").await?;
    on_event(Event::Ready);

    let mut misses = 0;
    while let Some(status) = statuses.next().await {
        let args = status.args()?;
        match (args.result, args.done) {
            ("verify-match", _) => {
                on_event(Event::Matched);
                break;
            }
            ("verify-no-match", true) => {
                misses += 1;
                device.verify_stop().await?;
                if misses >= ATTEMPTS {
                    on_event(Event::GaveUp);
                    break;
                }
                on_event(Event::NotRecognised);
                device.verify_start("any").await?;
            }
            // Short swipes and the like: the scan simply continues.
            (_, false) => {}
            (other, true) => {
                eprintln!("lock: fingerprint: {other}");
                on_event(Event::GaveUp);
                break;
            }
        }
    }
    let _ = device.verify_stop().await;
    let _ = device.release().await;
    Ok(())
}
