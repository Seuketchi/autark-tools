//! Clipboard history. A thread of its own watches the clipboard through wlr-data-control (GTK's clipboard needs a
//! focused window, which a background watcher doesn't have) and sends each text copied to the launcher, which keeps
//! the last few in memory. Nothing is written to disk, and anything a password manager marks as secret is skipped.

use std::io::Read;
use std::io::Write;
use std::os::fd::AsFd;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::{zwlr_data_control_device_v1, zwlr_data_control_manager_v1, zwlr_data_control_offer_v1, zwlr_data_control_source_v1};

/// How many copies are kept, and the most one may hold; a longer text is left out rather than cut short.
pub const KEEP: usize = 50;
pub const MAX_BYTES: usize = 100_000;

/// The text types, best first.
const TEXT: [&str; 4] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING"];
/// A mark password managers put on what they copy, so history leaves it alone.
const SECRET: [&str; 2] = ["x-kde-passwordManagerHint", "x-autark-secret"];

/// What the offers say they hold: their types, as they're announced.
type Types = Mutex<Vec<String>>;

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1>,
    sender: Option<Sender<String>>,
    /// What `copy` serves, and whether it's been replaced.
    text: Arc<String>,
    cancelled: bool,
}

/// Watches the clipboard until the process ends; each text copied is sent on the receiver.
pub fn watch() -> Receiver<String> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        if let Err(error) = run(sender) {
            eprintln!("launcher: clipboard history is off: {error}");
        }
    });
    receiver
}

fn run(sender: Sender<String>) -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|error| format!("no Wayland display: {error}"))?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut state = State { sender: Some(sender), ..State::default() };
    queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
    let seat = state.seat.clone().ok_or("the compositor has no seat")?;
    let manager = state.manager.clone().ok_or("the compositor doesn't offer wlr-data-control")?;
    let _device = manager.get_data_device(&seat, &handle, ());
    loop {
        queue.blocking_dispatch(&mut state).map_err(|error| format!("Wayland: {error}"))?;
        if state.cancelled {
            return Ok(());
        }
    }
}

/// Puts `text` on the clipboard. It's served from a thread of its own until something else is copied.
pub fn copy(text: String) -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|error| format!("no Wayland display: {error}"))?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut state = State { text: Arc::new(text), ..State::default() };
    queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
    let seat = state.seat.clone().ok_or("the compositor has no seat")?;
    let manager = state.manager.clone().ok_or("the compositor doesn't offer wlr-data-control, so nothing can be copied")?;
    let source = manager.create_data_source(&handle, ());
    for mime in TEXT {
        source.offer(mime.into());
    }
    let device = manager.get_data_device(&seat, &handle, ());
    device.set_selection(Some(&source));
    queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
    std::thread::spawn(move || {
        while !state.cancelled {
            if queue.blocking_dispatch(&mut state).is_err() {
                break;
            }
        }
        source.destroy();
        device.destroy();
        drop(connection);
    });
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, handle: &QueueHandle<Self>) {
        let wl_registry::Event::Global { name, interface, version } = event else { return };
        match interface.as_str() {
            "wl_seat" if state.seat.is_none() => state.seat = Some(registry.bind(name, version.min(1), handle, ())),
            "zwlr_data_control_manager_v1" => state.manager = Some(registry.bind(name, version.min(2), handle, ())),
            _ => {}
        }
    }
}

impl Dispatch<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, Types> for State {
    fn event(_: &mut Self, _: &zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, event: zwlr_data_control_offer_v1::Event, types: &Types, _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Ok(mut types) = types.lock()
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, event: zwlr_data_control_device_v1::Event, _: &(), connection: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_data_control_device_v1::Event::Selection { id: Some(offer) } => {
                let types = offer.data::<Types>().and_then(|types| types.lock().ok().map(|types| types.clone())).unwrap_or_default();
                let wanted = TEXT.iter().find(|mime| types.iter().any(|offered| offered == **mime));
                let secret = types.iter().any(|offered| SECRET.contains(&offered.as_str()));
                if let (Some(mime), false, Some(sender)) = (wanted, secret, state.sender.clone())
                    && let Ok((mut reader, writer)) = std::io::pipe()
                {
                    offer.receive((*mime).to_owned(), writer.as_fd());
                    // The request has to reach the app that owns the clipboard before it can answer.
                    let _ = connection.flush();
                    drop(writer);
                    // Read on a thread of its own: the owner may be slow, or be this very process.
                    std::thread::spawn(move || {
                        let mut bytes = Vec::new();
                        if reader.by_ref().take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes).is_ok()
                            && bytes.len() <= MAX_BYTES
                            && let Ok(text) = String::from_utf8(bytes)
                            && !text.trim().is_empty()
                        {
                            let _ = sender.send(text);
                        }
                    });
                }
                offer.destroy();
            }
            zwlr_data_control_device_v1::Event::Selection { id: None } => {}
            // A primary-selection offer (what's selected, not copied) isn't history.
            zwlr_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => offer.destroy(),
            zwlr_data_control_device_v1::Event::Finished => state.cancelled = true,
            _ => {}
        }
    }

    event_created_child!(State, zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, Mutex::new(Vec::new())),
    ]);
}

impl Dispatch<zwlr_data_control_source_v1::ZwlrDataControlSourceV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_data_control_source_v1::ZwlrDataControlSourceV1, event: zwlr_data_control_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } if TEXT.contains(&mime_type.as_str()) => {
                let _ = std::fs::File::from(fd).write_all(state.text.as_bytes());
            }
            zwlr_data_control_source_v1::Event::Cancelled => state.cancelled = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: zwlr_data_control_manager_v1::ZwlrDataControlManagerV1);

/// The first line of `text`, shortened to `max` characters, for a row's title.
pub fn title(text: &str, max: usize) -> String {
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>().trim_end())
    } else if text.trim().lines().count() > 1 {
        format!("{line} …")
    } else {
        line.to_owned()
    }
}

/// "just now", "5 min ago", "2 h ago", "3 d ago".
pub fn ago(seconds: u64) -> String {
    match seconds {
        0..=44 => "just now".to_owned(),
        45..=3599 => format!("{} min ago", seconds.div_ceil(60).max(1)),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(title("hello", 20), "hello");
        assert_eq!(title("  first line\nsecond", 20), "first line …");
        assert_eq!(title("\n\n  real start\n", 20), "real start");
        assert_eq!(title("abcdefghij", 4), "abcd…");
    }

    #[test]
    fn times_read_plainly() {
        assert_eq!(ago(3), "just now");
        assert_eq!(ago(120), "2 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(200_000), "2 d ago");
    }
}
