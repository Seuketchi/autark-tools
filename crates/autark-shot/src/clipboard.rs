//! The clipboard through wlr-data-control. GTK's clipboard needs a focused window and a capture bound to a key
//! has none, so the compositor would refuse it. The picture is served from a thread of its own, on its own
//! connection, until something else is copied.

use std::io::Write;
use std::thread::JoinHandle;

use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::{zwlr_data_control_device_v1, zwlr_data_control_manager_v1, zwlr_data_control_offer_v1, zwlr_data_control_source_v1};

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1>,
    png: Vec<u8>,
    cancelled: bool,
}

/// Puts `png` on the clipboard; the thread ends when another app takes the clipboard over.
pub fn serve(png: Vec<u8>) -> Result<JoinHandle<()>, String> {
    let connection = Connection::connect_to_env().map_err(|error| format!("no Wayland display: {error}"))?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut state = State { png, ..State::default() };
    queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
    let seat = state.seat.clone().ok_or("the compositor has no seat")?;
    let manager = state.manager.clone().ok_or("the compositor doesn't offer wlr-data-control, so nothing can be copied")?;
    let source = manager.create_data_source(&handle, ());
    source.offer("image/png".into());
    let device = manager.get_data_device(&seat, &handle, ());
    device.set_selection(Some(&source));
    queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
    Ok(std::thread::spawn(move || {
        while !state.cancelled {
            if queue.blocking_dispatch(&mut state).is_err() {
                break;
            }
        }
        source.destroy();
        device.destroy();
        drop(connection);
    }))
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

impl Dispatch<zwlr_data_control_source_v1::ZwlrDataControlSourceV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_data_control_source_v1::ZwlrDataControlSourceV1, event: zwlr_data_control_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } if mime_type == "image/png" => {
                let _ = std::fs::File::from(fd).write_all(&state.png);
            }
            zwlr_data_control_source_v1::Event::Cancelled => state.cancelled = true,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, event: zwlr_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            // Offers of what others copy, this one's own included; they're not read, only let go.
            zwlr_data_control_device_v1::Event::DataOffer { id } => id.destroy(),
            zwlr_data_control_device_v1::Event::Finished => state.cancelled = true,
            _ => {}
        }
    }

    event_created_child!(State, zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()),
    ]);
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: zwlr_data_control_manager_v1::ZwlrDataControlManagerV1);
delegate_noop!(State: ignore zwlr_data_control_offer_v1::ZwlrDataControlOfferV1);
