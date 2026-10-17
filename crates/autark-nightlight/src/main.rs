//! Night light: warms the screen from sunset to sunrise (or between set times) through Wayland gamma control,
//! fading in and out. `autark-nightlight daemon` runs it, started with the session; `on`, `off`, `toggle`,
//! `status` and `reload` talk to it over `org.autark.NightLight1`.

mod gamma;
mod schedule;
mod server;
mod sun;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Local;

use schedule::Config;
use server::{NightLight, Shared};

/// How often the temperature moves while fading, and the longest sleep otherwise (so a suspend or a clock
/// change is caught within a minute).
const FADE_STEP: Duration = Duration::from_secs(5);
const LONGEST_SLEEP: Duration = Duration::from_secs(60);

const USAGE: &str = "usage: autark-nightlight daemon|on|off|toggle|status|reload";

#[zbus::proxy(interface = "org.autark.NightLight1", default_service = "org.autark.NightLight1", default_path = "/org/autark/NightLight1")]
trait Control {
    fn set_active(&self, on: bool) -> zbus::Result<()>;
    fn toggle(&self) -> zbus::Result<()>;
    fn reload(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn active(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn temperature(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn schedule(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn manual(&self) -> zbus::Result<bool>;
}

fn main() {
    let command = std::env::args().nth(1).unwrap_or_default();
    let result = match command.as_str() {
        "daemon" => async_io::block_on(daemon()),
        "on" | "off" | "toggle" | "status" | "reload" => async_io::block_on(control(&command)).map_err(|error| match error {
            zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => "the night light daemon isn't running".to_owned(),
            error => error.to_string(),
        }),
        "-h" | "--help" => {
            println!("{USAGE}");
            return;
        }
        _ => Err(USAGE.to_owned()),
    };
    if let Err(error) = result {
        eprintln!("nightlight: {error}");
        std::process::exit(1);
    }
}

async fn control(command: &str) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let proxy = ControlProxy::new(&connection).await?;
    match command {
        "on" => proxy.set_active(true).await?,
        "off" => proxy.set_active(false).await?,
        "toggle" => proxy.toggle().await?,
        "reload" => proxy.reload().await?,
        _ => {
            let schedule = match proxy.schedule().await?.as_str() {
                "sun" => "sunset to sunrise",
                "custom" => "set times",
                _ => "no schedule",
            };
            let by_hand = if proxy.manual().await? { ", set by hand" } else { "" };
            match proxy.active().await? {
                true => println!("on, {} K ({schedule}{by_hand})", proxy.temperature().await?),
                false => println!("off ({schedule}{by_hand})"),
            }
        }
    }
    Ok(())
}

async fn daemon() -> Result<(), String> {
    let mut gamma = gamma::Gamma::connect()?;
    let (wake, woken) = async_channel::unbounded();
    let shared = Arc::new(Mutex::new(Shared { config: Config::load(), manual: None, active: false, temperature: schedule::DAY }));
    let service = NightLight { shared: shared.clone(), wake };
    let connection = zbus::connection::Builder::session()
        .name("org.autark.NightLight1")
        .serve_at(server::PATH, service)
        .build()
        .await
        .map_err(|error| format!("the session bus: {error} (is another night light running?)"))?;
    let interface = connection.object_server().interface::<_, NightLight>(server::PATH).await.map_err(|error| error.to_string())?;

    loop {
        let now = Local::now();
        let (kelvin, sleep, changed) = {
            let mut shared = shared.lock().unwrap();
            if let Some((_, Some(until))) = shared.manual
                && now >= until
            {
                shared.manual = None;
            }
            let (depth, next) = match shared.manual {
                Some((on, until)) => (if on { 1.0 } else { 0.0 }, until),
                None => (shared.config.depth(now), shared.config.next_change(now)),
            };
            let kelvin = schedule::temperature(depth, shared.config.night());
            let fading = depth > 0.0 && depth < 1.0;
            let sleep = if fading { FADE_STEP } else { next.and_then(|next| (next - now).to_std().ok()).unwrap_or(LONGEST_SLEEP).min(LONGEST_SLEEP) };
            let changed = (shared.active != (depth > 0.0), shared.temperature != kelvin);
            (shared.active, shared.temperature) = (depth > 0.0, kelvin);
            (kelvin, sleep.max(Duration::from_millis(200)), changed)
        };
        gamma.set(kelvin);
        let emitter = interface.signal_emitter();
        let service = interface.get().await;
        if changed.0 {
            let _ = service.active_changed(emitter).await;
            let _ = service.manual_changed(emitter).await;
        }
        if changed.1 {
            let _ = service.temperature_changed(emitter).await;
        }
        drop(service);

        let timer = std::pin::pin!(async_io::Timer::after(sleep));
        let command = std::pin::pin!(woken.recv());
        let wayland = std::pin::pin!(gamma.dispatch());
        match futures_util::future::select(futures_util::future::select(timer, command), wayland).await {
            futures_util::future::Either::Right((result, _)) => result?,
            futures_util::future::Either::Left(_) => {}
        }
    }
}
