//! `autark-lock --daemon`: locks when logind asks (`loginctl lock-session`, idle tools) and,
//! holding a sleep delay, before the laptop sleeps.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};

use futures_util::StreamExt;
use zbus::OwnedFd;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn get_session(&self, session_id: &str) -> zbus::Result<zbus::OwnedObjectPath>;
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1")]
trait Session {
    #[zbus(signal)]
    fn lock(&self) -> zbus::Result<()>;
}

pub async fn run(lock_on_sleep: bool) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let manager = ManagerProxy::new(&connection).await?;
    let session = SessionProxy::builder(&connection)
        .path(manager.get_session("auto").await?)
        .build()
        .await?;
    let mut locks = session.receive_lock().await?.fuse();
    let mut sleeps = manager.receive_prepare_for_sleep().await?.fuse();
    let inhibit = || manager.inhibit("sleep", "autark-lock", "Lock the screen before sleep", "delay");
    let mut delay = if lock_on_sleep { Some(inhibit().await?) } else { None };
    let mut child: Option<Child> = None;

    loop {
        futures_util::select! {
            signal = locks.next() => {
                if signal.is_none() {
                    break;
                }
                lock(&mut child);
            }
            signal = sleeps.next() => {
                let Some(signal) = signal else {
                    break;
                };
                if !lock_on_sleep {
                    continue;
                }
                if signal.args()?.start {
                    lock(&mut child);
                    // Sleep may go ahead once the screen is locked.
                    delay = None;
                } else {
                    delay = Some(inhibit().await?);
                }
            }
        }
    }
    drop(delay);
    Ok(())
}

/// Starts the lock screen unless it's already up, and waits until it reports the screen locked.
fn lock(child: &mut Option<Child>) {
    if child.as_mut().is_some_and(|child| child.try_wait().ok().flatten().is_none()) {
        return;
    }
    let Ok(program) = std::env::current_exe() else {
        return;
    };
    match Command::new(program).stdout(Stdio::piped()).spawn() {
        Ok(mut spawned) => {
            if let Some(stdout) = spawned.stdout.take() {
                let mut line = String::new();
                let _ = std::io::BufReader::new(stdout).read_line(&mut line);
            }
            *child = Some(spawned);
        }
        Err(err) => eprintln!("lock: starting the lock screen: {err}"),
    }
}
