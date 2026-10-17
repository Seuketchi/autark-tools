//! Displays: remembers how each set of monitors was arranged (modes, scale, position, rotation) and puts it
//! back when that set is plugged in again; closing the lid with another monitor connected turns the laptop
//! panel off. `autark-displays daemon` runs it, started with the session; `list` and `set` talk to it over
//! `org.autark.Displays1`, as Settings › Display will.

mod heads;
mod layout;
mod lid;
mod server;

use std::pin::pin;
use std::sync::{Arc, Mutex};

use futures_util::future::{Either, select};

use layout::{Head, Output, Saved};
use server::Displays;

const USAGE: &str = "usage: autark-displays daemon
       autark-displays list
       autark-displays set <connector or id> [--on|--off] [--mode 2560x1600@60] [--scale 1.5] [--position X,Y] [--transform normal|90|180|270|flipped…]";

#[zbus::proxy(interface = "org.autark.Displays1", default_service = "org.autark.Displays1", default_path = "/org/autark/Displays1")]
trait Control {
    fn list(&self) -> zbus::Result<String>;
    fn apply(&self, outputs: &str) -> zbus::Result<()>;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("daemon") => async_io::block_on(daemon()),
        Some("list") => async_io::block_on(list()),
        Some("set") => async_io::block_on(set(&args[1..])),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return;
        }
        _ => Err(USAGE.to_owned()),
    };
    if let Err(error) = result {
        eprintln!("displays: {error}");
        std::process::exit(1);
    }
}

fn bus_error(error: zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => "the displays daemon isn't running".into(),
        zbus::Error::MethodError(_, Some(message), _) => message,
        error => error.to_string(),
    }
}

async fn heads(proxy: &ControlProxy<'_>) -> Result<Vec<Head>, String> {
    let json = proxy.list().await.map_err(bus_error)?;
    serde_json::from_str(&json).map_err(|error| format!("the daemon's answer: {error}"))
}

async fn list() -> Result<(), String> {
    let connection = zbus::Connection::session().await.map_err(|error| error.to_string())?;
    let proxy = ControlProxy::new(&connection).await.map_err(bus_error)?;
    for head in heads(&proxy).await? {
        let built_in = if head.internal() { ", built in" } else { "" };
        println!("{}  {}{built_in}", head.name, head.id());
        match (&head.mode, head.enabled) {
            (Some(mode), true) => println!("  on: {} at {},{}, scale {}, {}", mode.text(), head.x, head.y, head.scale, head.transform),
            _ => println!("  off"),
        }
        let modes: Vec<String> = head.modes.iter().map(|mode| if mode.preferred { format!("{}*", mode.text()) } else { mode.text() }).collect();
        println!("  modes: {}", modes.join(", "));
    }
    Ok(())
}

async fn set(args: &[String]) -> Result<(), String> {
    let Some(target) = args.first() else { return Err(USAGE.to_owned()) };
    let connection = zbus::Connection::session().await.map_err(|error| error.to_string())?;
    let proxy = ControlProxy::new(&connection).await.map_err(bus_error)?;
    let heads = heads(&proxy).await?;
    let head = heads.iter().find(|head| head.name == *target || head.id() == *target).ok_or_else(|| format!("no monitor {target:?}; `autark-displays list` shows them"))?;
    let mut output = layout::current(head);
    let mut rest = args[1..].iter();
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--on" => output.enabled = true,
            "--off" => output.enabled = false,
            "--mode" => output.mode = Some(value()?),
            "--scale" => output.scale = value()?.parse().map_err(|_| "--scale takes a number like 1.5".to_owned())?,
            "--position" => {
                let text = value()?;
                let (x, y) = text.split_once(',').ok_or("--position takes X,Y")?;
                output.position = (x.trim().parse().map_err(|_| "--position takes X,Y")?, y.trim().parse().map_err(|_| "--position takes X,Y")?);
            }
            "--transform" => {
                let text = value()?;
                if !heads::TRANSFORMS.contains(&text.as_str()) {
                    return Err(format!("--transform takes one of {}", heads::TRANSFORMS.join(", ")));
                }
                output.transform = text;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let json = serde_json::to_string(&[output]).map_err(|error| error.to_string())?;
    proxy.apply(&json).await.map_err(bus_error)
}

fn save(saved: &Saved) {
    if let Err(error) = saved.save() {
        eprintln!("displays: saving {}: {error}", layout::path().display());
    }
}

enum Event {
    Wayland(Result<(), String>),
    Lid(bool),
    Request((Vec<Output>, server::Reply)),
}

async fn daemon() -> Result<(), String> {
    let mut outputs = heads::Outputs::connect()?;
    let mut saved = Saved::load();
    let shared = Arc::new(Mutex::new(String::from("[]")));
    // Both senders are kept here too, so neither channel closes when the lid watcher or the bus lets go.
    let (lid_changes, lid_changed) = async_channel::unbounded();
    let (requests, requested) = async_channel::unbounded();
    let connection = zbus::connection::Builder::session()
        .name("org.autark.Displays1")
        .serve_at(server::PATH, Displays { heads: shared.clone(), requests: requests.clone() })
        .build()
        .await
        .map_err(|error| format!("the session bus: {error} (is another displays daemon running?)"))?;
    let emitter = zbus::object_server::SignalEmitter::new(&connection, server::PATH).map_err(|error| error.to_string())?.into_owned();
    let watcher = lid_changes.clone();
    let lid = async move {
        lid::watch(watcher).await;
        futures_util::future::pending::<()>().await;
    };

    let main = async move {
        let _keep = (lid_changes, requests);
        let mut lid_closed = false;
        let mut last_key: Option<Vec<String>> = None;
        let mut react = true;
        // Tries at putting a set's layout back, so a compositor that won't take it isn't asked forever.
        let mut tries = 0;
        loop {
            // An apply reads the compositor's next report in with its answer, so go round until none is waiting.
            while outputs.take_changed() || react {
                react = false;
                let heads = outputs.heads();
                let key = layout::key(&heads, lid_closed);
                if last_key.as_ref() != Some(&key) {
                    last_key = Some(key.clone());
                    tries = 0;
                }
                match layout::plan(&saved, &heads, lid_closed) {
                    Ok(plan) if layout::differs(&heads, &plan) => {
                        if tries < 3 {
                            tries += 1;
                            if let Err(error) = outputs.apply(&plan).await {
                                eprintln!("displays: {error}");
                            }
                        }
                    }
                    Ok(_) => {
                        tries = 0;
                        if saved.find(&key).is_none() && saved.remember(&heads, lid_closed) {
                            save(&saved);
                        }
                    }
                    Err(error) => eprintln!("displays: {error}"),
                }
                let json = serde_json::to_string(&heads).unwrap_or_default();
                *shared.lock().unwrap() = json.clone();
                let _ = Displays::changed(&emitter, &json).await;
            }
            let event = {
                let wayland = pin!(outputs.dispatch());
                let lid = pin!(lid_changed.recv());
                let request = pin!(requested.recv());
                match select(select(wayland, lid), request).await {
                    Either::Left((Either::Left((result, _)), _)) => Event::Wayland(result),
                    Either::Left((Either::Right((closed, _)), _)) => Event::Lid(closed.unwrap_or(false)),
                    Either::Right((request, _)) => match request {
                        Ok(request) => Event::Request(request),
                        Err(_) => continue,
                    },
                }
            };
            match event {
                Event::Wayland(result) => result?,
                Event::Lid(closed) => {
                    react = closed != lid_closed;
                    lid_closed = closed;
                }
                Event::Request((wanted, reply)) => {
                    let heads = outputs.heads();
                    let result = match layout::settings(&heads, &wanted, true) {
                        Ok(settings) => match outputs.apply(&settings).await {
                            Ok(true) => {
                                // Remembered as asked for, not as next reported: Wayfire may reset it first.
                                if saved.remember(&layout::applied(&heads, &settings), lid_closed) {
                                    save(&saved);
                                }
                                tries = 0;
                                Ok(())
                            }
                            Ok(false) => Err("the monitors changed meanwhile; try again".into()),
                            Err(error) => Err(error),
                        },
                        Err(error) => Err(error),
                    };
                    let _ = reply.send(result).await;
                }
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    };
    match select(pin!(main), pin!(lid)).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Ok(()),
    }
}
