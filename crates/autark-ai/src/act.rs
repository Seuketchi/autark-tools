//! `Act`: runs the actions `Route` picked, once you've chosen them, through each tool's own interface:
//! volume through WirePlumber's `wpctl`, brightness, lock and sleep through logind, Wi-Fi through iwd, Focus
//! through the notification centre and night light through `autark-nightlight`. Apps open from their
//! desktop entries, files through `gio open`. The model never reaches here; only actions from the catalog,
//! and the three a suggestion offers, checked again.

use std::collections::HashMap;

use autark_common::backlight::Backlight;
use zbus::{OwnedObjectPath, OwnedVariant};

use crate::route::Action;

#[zbus::proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1", default_path = "/org/freedesktop/login1/session/auto")]
trait Session {
    fn lock(&self) -> zbus::Result<()>;
    fn set_brightness(&self, subsystem: &str, name: &str, brightness: u32) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.login1.Manager", default_service = "org.freedesktop.login1", default_path = "/org/freedesktop/login1")]
trait Login {
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "net.connman.iwd.Device", default_service = "net.connman.iwd")]
trait WifiDevice {
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.autark.Notifications1", default_service = "org.freedesktop.Notifications", default_path = "/org/autark/Notifications")]
trait Notifications {
    fn set_focus(&self, on: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.autark.NightLight1", default_service = "org.autark.NightLight1", default_path = "/org/autark/NightLight1")]
trait NightLight {
    fn set_active(&self, on: bool) -> zbus::Result<()>;
}

/// Runs each action and says how each went: empty for done, else why not, in the order given. Lock and
/// sleep go last, so "mute and lock" mutes first.
pub async fn run(session: &zbus::Connection, actions: &[Action]) -> Vec<String> {
    let mut order: Vec<usize> = (0..actions.len()).collect();
    order.sort_by_key(|at| matches!(actions[*at], Action::Lock | Action::Sleep));
    let mut results = vec![String::new(); actions.len()];
    let mut system = None;
    for at in order {
        results[at] = one(session, &mut system, &actions[at]).await.err().unwrap_or_default();
    }
    results
}

async fn one(session: &zbus::Connection, system: &mut Option<zbus::Connection>, action: &Action) -> Result<(), String> {
    match action {
        Action::SetVolume { percent } => wpctl(&["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{}%", (*percent).min(100))]).await,
        Action::Mute { on } => wpctl(&["set-mute", "@DEFAULT_AUDIO_SINK@", if *on { "1" } else { "0" }]).await,
        Action::SetBrightness { percent } => {
            let backlight = Backlight::find().ok_or("this laptop has no backlight to set")?;
            let value = backlight.value(f64::from((*percent).clamp(5, 100)));
            let connection = system_bus(system).await?;
            let result = async { SessionProxy::new(&connection).await?.set_brightness("backlight", &backlight.name, value).await };
            result.await.map_err(|error| said("logind", error))
        }
        Action::Wifi { on } => {
            let connection = system_bus(system).await?;
            let devices = wifi_devices(&connection).await.map_err(|error| said("iwd", error))?;
            if devices.is_empty() {
                return Err("there's no Wi-Fi device".to_owned());
            }
            for device in devices {
                let result = async { WifiDeviceProxy::builder(&connection).path(device).build().await?.set_powered(*on).await };
                result.await.map_err(|error| said("iwd", error))?;
            }
            Ok(())
        }
        Action::Focus { on } => {
            let result = async { NotificationsProxy::new(session).await?.set_focus(*on).await };
            result.await.map_err(|error| said("the notification centre", error))
        }
        Action::NightLight { on } => {
            let result = async { NightLightProxy::new(session).await?.set_active(*on).await };
            result.await.map_err(|error| said("autark-nightlight", error))
        }
        Action::Lock => {
            let connection = system_bus(system).await?;
            let result = async { SessionProxy::new(&connection).await?.lock().await };
            result.await.map_err(|error| said("logind", error))
        }
        Action::Sleep => {
            let connection = system_bus(system).await?;
            let result = async { LoginProxy::new(&connection).await?.suspend(true).await };
            result.await.map_err(|error| said("logind", error))
        }
        Action::OpenApp { name } => open(name),
        Action::OpenUrl { url } => spawn("gio", &["open".to_owned(), web_address(url)?]),
        Action::QuitApp { app } => quit(app),
        Action::OpenFile { path } => {
            let path = openable(path)?;
            spawn("gio", &["open".to_owned(), path.to_string_lossy().into_owned()])
        }
        Action::OpenSettings { pane } => {
            if !PANES.contains(&pane.as_str()) {
                return Err(format!("Settings has no pane “{pane}”"));
            }
            spawn("autark-settings", std::slice::from_ref(pane))
        }
    }
}

async fn system_bus(system: &mut Option<zbus::Connection>) -> Result<zbus::Connection, String> {
    if let Some(connection) = system {
        return Ok(connection.clone());
    }
    let connection = zbus::Connection::system().await.map_err(|error| format!("the system bus: {error}"))?;
    *system = Some(connection.clone());
    Ok(connection)
}

async fn wifi_devices(connection: &zbus::Connection) -> zbus::Result<Vec<OwnedObjectPath>> {
    let objects = zbus::fdo::ObjectManagerProxy::builder(connection).destination("net.connman.iwd").path("/").build().await?.get_managed_objects().await?;
    let devices = objects
        .into_iter()
        .filter(|(_, interfaces): &(OwnedObjectPath, std::collections::BTreeMap<zbus::names::OwnedInterfaceName, HashMap<String, OwnedVariant>>)| {
            interfaces.keys().any(|name| name.as_str() == "net.connman.iwd.Device")
        })
        .map(|(path, _)| path)
        .collect();
    Ok(devices)
}

/// WirePlumber owns the hardware volume, so a volume set on the PipeWire node directly doesn't stick.
async fn wpctl(args: &[&str]) -> Result<(), String> {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let status = blocking::unblock(move || std::process::Command::new("wpctl").args(&args).status()).await.map_err(|error| format!("wpctl: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("wpctl failed ({status})")) }
}

fn open(name: &str) -> Result<(), String> {
    let app = autark_common::desktop::find_app(name).ok_or_else(|| format!("no app called “{name}”"))?;
    let (program, args) = app.command.split_first().ok_or_else(|| format!("{} has no command", app.name))?;
    spawn(program, args).map_err(|error| format!("{}: {error}", app.name))
}

/// A web address that is safe to hand to the browser: http or https only, with a bare name such as `youtube.com`
/// becoming https; no spaces or control characters, no user name before the host, and a host with a dot.
fn web_address(text: &str) -> Result<String, String> {
    let text = text.trim();
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("https".to_owned(), text),
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let name = host.split(':').next().unwrap_or_default();
    if scheme != "http" && scheme != "https" {
        return Err(format!("only http and https addresses open, not “{text}”"));
    }
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) || host.contains('@') || !name.contains('.') || name.starts_with('.') || name.ends_with('.') {
        return Err(format!("“{text}” isn't a web address"));
    }
    Ok(format!("{scheme}://{rest}"))
}

/// Starts a program in a process group of its own, so it outlives `autark-ai` restarting.
fn spawn(program: &str, args: &[String]) -> Result<(), String> {
    std::os::unix::process::CommandExt::process_group(
        std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()),
        0,
    )
    .spawn()
    .map(drop)
    .map_err(|error| format!("{program}: {error}"))
}

/// The panes `autark-settings <pane>` opens.
const PANES: [&str; 11] = ["wifi", "bluetooth", "appearance", "dock", "display", "sound", "power", "lock", "keyboard", "ai", "about"];

/// Asks an app to quit, as closing its windows would: a TERM to each process in its cgroup, read now, so one
/// it started since is asked too. Only apps you started, under `app.slice`, and never `autark-ai` itself.
fn quit(app: &str) -> Result<(), String> {
    if !app.starts_with("/user.slice/") || !app.contains("/app.slice/") || app.split('/').any(|part| part == "..") {
        return Err(format!("{app} isn't an app you started"));
    }
    let procs = std::fs::read_to_string(format!("/sys/fs/cgroup{app}/cgroup.procs")).map_err(|_| "that app isn't running any more".to_owned())?;
    let me = std::process::id().to_string();
    let pids: Vec<String> = procs.split_whitespace().filter(|pid| *pid != me).map(str::to_owned).collect();
    if pids.is_empty() {
        return Err("that app isn't running any more".to_owned());
    }
    let status = std::process::Command::new("kill").arg("-TERM").args(&pids).status().map_err(|error| format!("kill: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("kill failed ({status})")) }
}

/// A file Kith may open: one it may read (see `files`), or an `autark-pkg` build log.
fn openable(path: &str) -> Result<std::path::PathBuf, String> {
    let config: crate::config::Ai = autark_common::config::load("ai");
    let logs = std::fs::canonicalize(&config.suggestions.build_logs).ok();
    if let Ok(real) = std::fs::canonicalize(path)
        && logs.is_some()
        && real.parent() == logs.as_deref()
        && real.extension().is_some_and(|extension| extension == "log")
    {
        return Ok(real);
    }
    crate::files::allowed(path).map_err(|why| format!("{path}: {why}"))
}

/// A D-Bus error in words, naming the service when it isn't there.
fn said(service: &str, error: zbus::Error) -> String {
    match &error {
        zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" || name.as_str().starts_with("org.freedesktop.DBus.Error.Spawn.") => {
            format!("{service} isn't running")
        }
        zbus::Error::FDO(inner) if matches!(**inner, zbus::fdo::Error::ServiceUnknown(_)) => format!("{service} isn't running"),
        zbus::Error::MethodError(_, Some(message), _) => format!("{service}: {message}"),
        _ => format!("{service}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_name_becomes_https_and_only_web_addresses_open() {
        assert_eq!(web_address("youtube.com").unwrap(), "https://youtube.com");
        assert_eq!(web_address(" https://github.com/Seuketchi ").unwrap(), "https://github.com/Seuketchi");
        assert_eq!(web_address("HTTP://example.org/a?b=c#d").unwrap(), "http://example.org/a?b=c#d");
        assert_eq!(web_address("localhost.localdomain:8080/x").unwrap(), "https://localhost.localdomain:8080/x");
        for bad in ["youtube", "file:///etc/passwd", "javascript:alert(1)", "ftp://x.org", "https://google.com@evil.org", "https://a b.com", "", "https://.com", "evil.\ncom"] {
            assert!(web_address(bad).is_err(), "{bad:?} should not open");
        }
    }
}
