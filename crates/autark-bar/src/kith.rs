//! Kith's small Visor, shown while AI is on in ai.toml: it pulses while Kith thinks or answers, has a dot
//! while a suggestion is open, and opens the Assistant panel when clicked.

use std::time::Duration;

use autark_common::visor::{State, Visor};
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;
use serde::Deserialize;

use crate::shared::Shared;

#[zbus::proxy(
    interface = "org.autark.AI1",
    default_service = "org.autark.AI1",
    default_path = "/org/autark/AI1"
)]
trait Ai {
    fn open_suggestions(&self) -> zbus::Result<String>;
    #[zbus(property(emits_changed_signal = "false"))]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(signal)]
    fn state_changed(&self, state: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn history_changed(&self) -> zbus::Result<()>;
}

/// Only ai.toml's switch matters here.
#[derive(Deserialize, Default)]
#[serde(default)]
struct AiToml {
    enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub struct Kith {
    pub on: bool,
    pub state: State,
    pub suggestion: bool,
}

/// Kith as the bar shows it, watched once for every bar.
pub fn source() -> Shared<Kith> {
    let on = autark_common::config::load::<AiToml>("ai").enabled;
    let shared = Shared::new(Kith {
        on,
        ..Kith::default()
    });
    let watched = shared.clone();
    let watch = autark_common::watch::config("ai", move || {
        let on = autark_common::config::load::<AiToml>("ai").enabled;
        watched.set(Kith {
            on,
            ..watched.get()
        });
    });
    std::mem::forget(watch);
    glib::spawn_future_local({
        let shared = shared.clone();
        async move {
            loop {
                if let Err(err) = follow(&shared).await {
                    eprintln!("kith: {err}");
                }
                shared.set(Kith {
                    state: State::Idle,
                    suggestion: false,
                    ..shared.get()
                });
                // autark-ai may start later, or restart.
                glib::timeout_future(Duration::from_secs(10)).await;
            }
        }
    });
    shared
}

async fn follow(shared: &Shared<Kith>) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let ai = AiProxy::new(&connection).await?;
    let mut states = ai.receive_state_changed().await?;
    let mut changes = ai.receive_history_changed().await?;
    let open = |json: &str| {
        serde_json::from_str::<Vec<serde_json::Value>>(json).is_ok_and(|open| !open.is_empty())
    };
    shared.set(Kith {
        state: State::from_word(&ai.state().await?),
        suggestion: open(&ai.open_suggestions().await?),
        ..shared.get()
    });
    loop {
        let state = std::pin::pin!(states.next());
        let changed = std::pin::pin!(changes.next());
        match futures_util::future::select(state, changed).await {
            futures_util::future::Either::Left((Some(signal), _)) => shared.set(Kith {
                state: State::from_word(&signal.args()?.state),
                ..shared.get()
            }),
            futures_util::future::Either::Right((Some(_), _)) => {
                let suggestion = open(&ai.open_suggestions().await?);
                shared.set(Kith {
                    suggestion,
                    ..shared.get()
                });
            }
            _ => return Ok(()),
        }
    }
}

pub fn view(source: &Shared<Kith>) -> gtk::Button {
    let visor = Visor::new(22);
    visor.set_blinks(false);
    let dot = gtk::Box::builder()
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .build();
    dot.add_css_class("kith-dot");
    let overlay = gtk::Overlay::builder().child(visor.widget()).build();
    overlay.add_overlay(&dot);
    let button = gtk::Button::builder()
        .child(&overlay)
        .tooltip_text("Kith")
        .build();
    button.add_css_class("item");
    button.add_css_class("icon");
    button.connect_clicked(|_| open_panel());
    source.watch(&button, move |button, kith| {
        button.set_visible(kith.on);
        visor.set_state(kith.state);
        visor.set_pulse(kith.state != State::Idle);
        dot.set_visible(kith.suggestion);
        button.set_tooltip_text(Some(match (kith.state, kith.suggestion) {
            (State::Thinking, _) => "Kith is thinking",
            (State::Speaking, _) => "Kith is answering",
            (_, true) => "Kith has a suggestion",
            _ => "Kith",
        }));
    });
    button
}

/// The Assistant panel, opened or closed by the launcher.
fn open_panel() {
    let mut command = std::process::Command::new("autark-launcher");
    command
        .arg("--assistant")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match std::os::unix::process::CommandExt::process_group(&mut command, 0).spawn() {
        // Waited for, so it doesn't linger as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(err) => eprintln!("kith: opening the Assistant panel: {err}"),
    }
}
