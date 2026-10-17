//! The focused app's name, from the compositor's wlr-foreign-toplevel list (Wayfire's
//! `foreign-toplevel` plugin). Hidden when nothing has focus or the compositor lacks the protocol.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

pub fn new() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("item");
    label.add_css_class("app-name");
    label.set_visible(false);
    if let Err(err) = watch(label.clone()) {
        eprintln!("app name: {err}");
    }
    label
}

#[derive(Clone, Default)]
struct Toplevel {
    app_id: Option<String>,
    activated: bool,
}

struct State {
    label: gtk::Label,
    /// Applied on each handle's `done` event, as the protocol asks.
    pending: HashMap<ObjectId, Toplevel>,
    toplevels: HashMap<ObjectId, Toplevel>,
    names: HashMap<String, String>,
}

impl State {
    /// The handle's state to change before its next `done`, starting from what it shows now.
    fn pending(&mut self, id: &ObjectId) -> &mut Toplevel {
        let current = self.toplevels.get(id).cloned().unwrap_or_default();
        self.pending.entry(id.clone()).or_insert(current)
    }

    fn show_focused(&mut self) {
        let app_id = self
            .toplevels
            .values()
            .find(|toplevel| toplevel.activated)
            .and_then(|toplevel| toplevel.app_id.clone());
        match app_id {
            Some(app_id) => {
                let name = self.names.entry(app_id.clone()).or_insert_with(|| {
                    autark_common::desktop::app_name(&app_id).unwrap_or_else(|| fallback_name(&app_id))
                });
                self.label.set_label(name);
                self.label.set_visible(true);
            }
            None => self.label.set_visible(false),
        }
    }
}

/// "org.example.TextEditor" reads as "TextEditor", "foot" as "Foot".
fn fallback_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A Wayland connection of its own, read from the GTK main loop whenever the compositor writes.
fn watch(label: gtk::Label) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let mut queue: EventQueue<State> = connection.new_event_queue();
    connection.display().get_registry(&queue.handle(), ());
    let mut state = State { label, pending: HashMap::new(), toplevels: HashMap::new(), names: HashMap::new() };
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;

    let fd = connection.backend().poll_fd().as_raw_fd();
    let queue = Rc::new(RefCell::new(queue));
    let state = Rc::new(RefCell::new(state));
    glib_unix::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
        let mut queue = queue.borrow_mut();
        if let Some(guard) = queue.prepare_read() {
            let _ = guard.read();
        }
        match queue.dispatch_pending(&mut state.borrow_mut()).and(queue.flush().map_err(Into::into)) {
            Ok(_) => glib::ControlFlow::Continue,
            Err(err) => {
                eprintln!("app name: compositor connection lost: {err}");
                state.borrow().label.set_visible(false);
                glib::ControlFlow::Break
            }
        }
    });
    Ok(())
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
        if let wl_registry::Event::Global { name, interface, version } = event
            && interface == ZwlrForeignToplevelManagerV1::interface().name
        {
            registry.bind::<ZwlrForeignToplevelManagerV1, _, _>(name, version.min(3), queue, ());
        }
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
            state.pending.insert(toplevel.id(), Toplevel::default());
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
            Event::AppId { app_id } => state.pending(&id).app_id = Some(app_id),
            Event::State { state: states } => {
                let activated = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_ne_bytes(*bytes))
                    .any(|value| value == zwlr_foreign_toplevel_handle_v1::State::Activated as u32);
                state.pending(&id).activated = activated;
            }
            Event::Done => {
                if let Some(toplevel) = state.pending.remove(&id) {
                    state.toplevels.insert(id, toplevel);
                }
                state.show_focused();
            }
            Event::Closed => {
                state.pending.remove(&id);
                state.toplevels.remove(&id);
                handle.destroy();
                state.show_focused();
            }
            _ => {}
        }
    }
}
