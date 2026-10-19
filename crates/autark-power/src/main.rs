//! Idle timers and the low-battery warning. After `power.toml`'s minutes with no one at the laptop the screens
//! go off (back on at a touch), and later it sleeps through logind; the battery's timers apply while UPower
//! says it's on battery, the plugged-in ones otherwise. An app holding an idle inhibitor (a playing video)
//! holds both off. Once the screens are off it locks, at once or after `lock_after`. When the battery falls
//! to the warning level, a notification says so, once per discharge. `power.toml` is watched, so Settings'
//! changes apply at once. Start it with the session.

mod config;
mod idle;

use futures_util::StreamExt;
use futures_util::future::{Either, select};

use config::Power;
use idle::{Event, Idle, Timer};

#[zbus::proxy(interface = "org.freedesktop.UPower", default_service = "org.freedesktop.UPower", default_path = "/org/freedesktop/UPower")]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(interface = "org.freedesktop.UPower.Device", default_service = "org.freedesktop.UPower", default_path = "/org/freedesktop/UPower/devices/DisplayDevice")]
trait Device {
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(interface = "org.freedesktop.login1.Manager", default_service = "org.freedesktop.login1", default_path = "/org/freedesktop/login1")]
trait Manager {
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
    fn get_session(&self, session_id: &str) -> zbus::Result<zbus::OwnedObjectPath>;
}

#[zbus::proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1")]
trait Session {
    fn lock(&self) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.Notifications", default_service = "org.freedesktop.Notifications", default_path = "/org/freedesktop/Notifications")]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: std::collections::HashMap<&str, zbus::Variant<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

enum Woken {
    Idle(Vec<Event>),
    /// Long enough with the screens off to lock.
    Lock,
    Config,
    Supply(Option<Supply>),
    /// Asked to stop (SIGTERM, SIGINT, SIGHUP).
    Stop,
}

/// UPower's `State` while the battery runs down.
const DISCHARGING: u32 = 2;

/// What UPower says: whether it's on battery, and the battery's charge if there is one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Supply {
    on_battery: bool,
    battery: Option<(f64, bool)>,
}

fn main() {
    if let Err(error) = async_io::block_on(daemon()) {
        eprintln!("power: {error}");
        std::process::exit(1);
    }
}

async fn daemon() -> Result<(), String> {
    let mut idle = Idle::connect()?;
    let mut power = Power::load();
    let mut changes = autark_common::changes::Changes::config("power").map_err(|error| format!("watching power.toml: {error}"))?;
    // Stopping returns normally, so the screens come back on as `idle` is dropped.
    let mut signals = async_signal::Signals::new([async_signal::Signal::Term, async_signal::Signal::Int, async_signal::Signal::Hup]).map_err(|error| format!("signals: {error}"))?;
    let (supplies, supplied) = async_channel::unbounded();
    let watcher = supplies.clone();
    let upower = async move {
        watch(watcher).await;
        futures_util::future::pending::<()>().await;
    };

    let main = async move {
        let _keep = supplies;
        let mut supply = Supply::default();
        let mut warned = false;
        // When to lock, while the screens are off.
        let mut lock_at: Option<std::time::Instant> = None;
        idle.set(power.timeouts(supply.on_battery).0, power.timeouts(supply.on_battery).1);
        loop {
            // What woke it, with the waits let go before it's handled.
            let woken = {
                let wayland = std::pin::pin!(idle.dispatch());
                let changed = std::pin::pin!(changes.next());
                let supplied = std::pin::pin!(supplied.recv());
                let locking = std::pin::pin!(async {
                    match lock_at {
                        Some(at) => async_io::Timer::at(at).await,
                        None => futures_util::future::pending().await,
                    };
                });
                let stop = std::pin::pin!(signals.next());
                match select(select(select(wayland, locking), select(changed, supplied)), stop).await {
                    Either::Left((Either::Left((Either::Left((events, _)), _)), _)) => Woken::Idle(events?),
                    Either::Left((Either::Left((Either::Right(_), _)), _)) => Woken::Lock,
                    Either::Left((Either::Right((Either::Left((changed, _)), _)), _)) => {
                        changed.map_err(|error| format!("watching power.toml: {error}"))?;
                        Woken::Config
                    }
                    Either::Left((Either::Right((Either::Right((update, _)), _)), _)) => Woken::Supply(update.ok()),
                    Either::Right(_) => Woken::Stop,
                }
            };
            let mut reset = false;
            match woken {
                Woken::Idle(events) => {
                    for event in events {
                        match event {
                            Event::Idle(Timer::ScreenOff) => {
                                idle.screens(true);
                                lock_at = power.lock_delay().map(|delay| std::time::Instant::now() + delay);
                            }
                            Event::Back(Timer::ScreenOff) => {
                                idle.screens(false);
                                lock_at = None;
                            }
                            Event::Idle(Timer::Sleep) => suspend().await,
                            Event::Back(Timer::Sleep) => {}
                        }
                    }
                }
                Woken::Lock => {
                    lock_at = None;
                    lock().await;
                }
                Woken::Config => {
                    let loaded = Power::load();
                    reset = loaded.timeouts(supply.on_battery) != power.timeouts(supply.on_battery);
                    power = loaded;
                    warned = warn(&power, supply, warned).await;
                }
                Woken::Supply(Some(update)) => {
                    reset = update.on_battery != supply.on_battery;
                    supply = update;
                    warned = warn(&power, supply, warned).await;
                }
                Woken::Supply(None) => {}
                Woken::Stop => return Ok(()),
            }
            if reset {
                let (screen_off, sleep) = power.timeouts(supply.on_battery);
                idle.set(screen_off, sleep);
            }
        }
    };
    match select(std::pin::pin!(upower), std::pin::pin!(main)).await {
        Either::Left(_) => Ok(()),
        Either::Right((result, _)) => result,
    }
}

/// Sends what UPower says, now and whenever it changes. Without UPower it's plugged in, with no battery.
async fn watch(supplies: async_channel::Sender<Supply>) {
    let Ok(connection) = zbus::Connection::system().await else { return };
    let (Ok(upower), Ok(device)) = (UPowerProxy::new(&connection).await, DeviceProxy::new(&connection).await) else { return };
    let mut on_battery = upower.receive_on_battery_changed().await;
    let mut percentage = device.receive_percentage_changed().await;
    let mut state = device.receive_state_changed().await;
    loop {
        let battery = match device.is_present().await {
            Ok(true) => Some((device.percentage().await.unwrap_or(100.0), device.state().await.unwrap_or(0) == DISCHARGING)),
            _ => None,
        };
        let supply = Supply { on_battery: upower.on_battery().await.unwrap_or(false), battery };
        if supplies.send(supply).await.is_err() {
            return;
        }
        let any = select(on_battery.next(), select(percentage.next(), state.next()));
        if let Either::Left((None, _)) = any.await {
            return;
        }
    }
}

/// Warns once when the battery reaches `low_battery` while running down; plugging in starts it again.
/// Returns whether it has warned this time.
async fn warn(power: &Power, supply: Supply, warned: bool) -> bool {
    let (now, warned) = low(power, supply, warned);
    if let Some(percentage) = now {
        let sent = async {
            let connection = zbus::Connection::session().await?;
            let notifications = NotificationsProxy::new(&connection).await?;
            let hints = std::collections::HashMap::from([("urgency", zbus::Variant::U8(2))]);
            notifications.notify("Power", 0, "battery-caution-symbolic", "Battery low", &format!("{percentage:.0}% left. Plug in soon."), &[], hints, -1).await
        };
        if let Err(error) = sent.await {
            eprintln!("power: low battery warning: {error}");
        }
    }
    warned
}

/// Whether to warn now (with the charge), and whether it counts as warned afterwards.
fn low(power: &Power, supply: Supply, warned: bool) -> (Option<f64>, bool) {
    match supply.battery {
        Some((percentage, true)) if power.low_battery > 0 && percentage <= f64::from(power.low_battery) => ((!warned).then_some(percentage), true),
        _ => (None, false),
    }
}

/// Locks this session through logind, which `autark-lock --daemon` answers.
async fn lock() {
    let locked = async {
        let connection = zbus::Connection::system().await?;
        let path = ManagerProxy::new(&connection).await?.get_session("auto").await?;
        SessionProxy::builder(&connection).path(path).build().await?.lock().await
    };
    if let Err(error) = locked.await {
        eprintln!("power: locking: {error}");
    }
}

async fn suspend() {
    let slept = async { ManagerProxy::new(&zbus::Connection::system().await?).await?.suspend(false).await };
    if let Err(error) = slept.await {
        eprintln!("power: sleeping: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_once_per_discharge() {
        let power = Power::default();
        let at = |percentage, discharging| Supply { on_battery: discharging, battery: Some((percentage, discharging)) };
        assert_eq!(low(&power, at(30.0, true), false), (None, false));
        assert_eq!(low(&power, at(10.0, true), false), (Some(10.0), true));
        assert_eq!(low(&power, at(8.0, true), true), (None, true));
        assert_eq!(low(&power, at(8.0, false), true), (None, false), "plugging in starts it again");
        assert_eq!(low(&Power { low_battery: 0, ..power }, at(3.0, true), false), (None, false));
        assert_eq!(low(&power, Supply::default(), false), (None, false), "no battery");
    }
}
