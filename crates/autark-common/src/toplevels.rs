//! Open windows from the compositor's wlr-foreign-toplevel list (Wayfire's `foreign-toplevel`
//! plugin), read on the GLib main loop: their app IDs, titles and focus, and focusing one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::rc::Rc;

use glib::ControlFlow;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    /// Stable while the window is open.
    pub id: u32,
    pub app_id: String,
    pub title: String,
    pub activated: bool,
    pub minimized: bool,
}

/// A handle for focusing windows; dropping it doesn't stop the watching.
#[derive(Clone)]
pub struct Toplevels {
    connection: Connection,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    handles: HashMap<ObjectId, ZwlrForeignToplevelHandleV1>,
    /// Applied on each handle's `done` event, as the protocol asks.
    pending: HashMap<ObjectId, Window>,
    windows: HashMap<ObjectId, Window>,
    changed: bool,
}

impl State {
    /// The window's state to change before its next `done`, starting from what it shows now.
    fn pending(&mut self, id: &ObjectId) -> &mut Window {
        let current = self.windows.get(id).cloned().unwrap_or_else(|| Window {
            id: id.protocol_id(),
            ..Window::default()
        });
        self.pending.entry(id.clone()).or_insert(current)
    }

    fn snapshot(&self) -> Vec<Window> {
        let mut windows: Vec<Window> = self.windows.values().cloned().collect();
        windows.sort_by_key(|window| window.id);
        windows
    }
}

/// Connects to the compositor and calls `on_change` with every open window, now and whenever
/// one opens, closes, changes title or takes focus.
pub fn watch(on_change: impl Fn(&[Window]) + 'static) -> Result<Toplevels, Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let mut queue: EventQueue<State> = connection.new_event_queue();
    connection.display().get_registry(&queue.handle(), ());
    let state = Rc::new(RefCell::new(State::default()));
    queue.roundtrip(&mut state.borrow_mut())?;
    queue.roundtrip(&mut state.borrow_mut())?;
    on_change(&state.borrow().snapshot());

    let fd = connection.backend().poll_fd().as_raw_fd();
    let queue = RefCell::new(queue);
    let watched = state.clone();
    glib_unix::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
        let mut queue = queue.borrow_mut();
        if let Some(guard) = queue.prepare_read() {
            let _ = guard.read();
        }
        let dispatched = queue.dispatch_pending(&mut watched.borrow_mut());
        if let Err(err) = dispatched.and(queue.flush().map_err(Into::into)) {
            eprintln!("toplevels: compositor connection lost: {err}");
            on_change(&[]);
            return ControlFlow::Break;
        }
        // Called with the state released, so `on_change` may focus a window.
        let changed = std::mem::take(&mut watched.borrow_mut().changed);
        if changed {
            let windows = watched.borrow().snapshot();
            on_change(&windows);
        }
        ControlFlow::Continue
    });
    Ok(Toplevels { connection, state })
}

impl Toplevels {
    /// Minimises the window, or restores it without focusing it.
    pub fn set_minimized(&self, id: u32, minimized: bool) {
        let state = self.state.borrow();
        if let Some(handle) = state.handles.values().find(|handle| handle.id().protocol_id() == id) {
            if minimized {
                handle.set_minimized();
            } else {
                handle.unset_minimized();
            }
            let _ = self.connection.flush();
        }
    }

    /// Focuses the window, raising it and restoring it if minimised.
    pub fn activate(&self, id: u32) {
        let state = self.state.borrow();
        let Some(seat) = &state.seat else {
            return;
        };
        if let Some(handle) = state.handles.values().find(|handle| handle.id().protocol_id() == id) {
            handle.activate(seat);
            let _ = self.connection.flush();
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        _: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == ZwlrForeignToplevelManagerV1::interface().name {
                registry.bind::<ZwlrForeignToplevelManagerV1, _, _>(name, version.min(3), queue, ());
            } else if interface == wl_seat::WlSeat::interface().name {
                registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), queue, ());
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.seat.get_or_insert_with(|| seat.clone());
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.pending(&toplevel.id());
            state.handles.insert(toplevel.id(), toplevel);
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let id = handle.id();
        match event {
            Event::AppId { app_id } => state.pending(&id).app_id = app_id,
            Event::Title { title } => state.pending(&id).title = title,
            Event::State { state: states } => {
                let has = |wanted: zwlr_foreign_toplevel_handle_v1::State| states.as_chunks::<4>().0.iter().map(|bytes| u32::from_ne_bytes(*bytes)).any(|value| value == wanted as u32);
                let pending = state.pending(&id);
                pending.activated = has(zwlr_foreign_toplevel_handle_v1::State::Activated);
                pending.minimized = has(zwlr_foreign_toplevel_handle_v1::State::Minimized);
            }
            Event::Done => {
                if let Some(window) = state.pending.remove(&id) {
                    state.windows.insert(id, window);
                    state.changed = true;
                }
            }
            Event::Closed => {
                state.pending.remove(&id);
                state.windows.remove(&id);
                state.handles.remove(&id);
                handle.destroy();
                state.changed = true;
            }
            _ => {}
        }
    }
}
