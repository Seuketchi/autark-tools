//! Warming every output through `wlr-gamma-control`. A table holds only while its control lives, so the daemon
//! keeps the connection; in daylight it lets go, and the compositor puts the original ramps back.

use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsFd, OwnedFd};

use async_io::Async;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::gamma_control::v1::client::{zwlr_gamma_control_manager_v1, zwlr_gamma_control_v1};

use crate::schedule::{self, DAY};

struct Output {
    /// The registry name, which says which output a control or a removal is about.
    global: u32,
    wl: wl_output::WlOutput,
    control: Option<zwlr_gamma_control_v1::ZwlrGammaControlV1>,
    size: Option<u32>,
    applied: Option<u32>,
    /// The output's gamma was refused; it's asked again the next night.
    refused: bool,
}

#[derive(Default)]
struct State {
    manager: Option<zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1>,
    outputs: Vec<Output>,
}

pub struct Gamma {
    connection: Connection,
    queue: EventQueue<State>,
    handle: QueueHandle<State>,
    state: State,
    fd: Async<OwnedFd>,
    kelvin: u32,
}

impl Gamma {
    pub fn connect() -> Result<Self, String> {
        let connection = Connection::connect_to_env().map_err(|error| format!("no Wayland display: {error}"))?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
        if state.manager.is_none() {
            return Err("the compositor doesn't offer wlr-gamma-control (Wayfire and other wlroots compositors have it)".into());
        }
        let fd = connection.as_fd().try_clone_to_owned().map_err(|error| format!("Wayland: {error}"))?;
        let fd = Async::new_nonblocking(fd).map_err(|error| format!("Wayland: {error}"))?;
        Ok(Self { connection, queue, handle, state, fd, kelvin: DAY })
    }

    /// Sets every output to `kelvin`, now and as outputs come and go.
    pub fn set(&mut self, kelvin: u32) {
        self.kelvin = kelvin;
        self.apply();
    }

    fn apply(&mut self) {
        let Some(manager) = &self.state.manager else { return };
        for output in &mut self.state.outputs {
            if self.kelvin == DAY {
                if let Some(control) = output.control.take() {
                    control.destroy();
                }
                (output.size, output.applied, output.refused) = (None, None, false);
                continue;
            }
            if output.control.is_none() && !output.refused {
                output.control = Some(manager.get_gamma_control(&output.wl, &self.handle, output.global));
            }
            if let (Some(control), Some(size)) = (&output.control, output.size)
                && output.applied != Some(self.kelvin)
            {
                match table(size as usize, self.kelvin) {
                    Ok(file) => {
                        control.set_gamma(file.as_fd());
                        output.applied = Some(self.kelvin);
                    }
                    Err(error) => eprintln!("nightlight: gamma table: {error}"),
                }
            }
        }
        let _ = self.connection.flush();
    }

    /// Waits for the compositor to say something and handles it: outputs plugged in or out, table sizes.
    pub async fn dispatch(&mut self) -> Result<(), String> {
        let failed = |error: &dyn std::fmt::Display| format!("Wayland: {error}");
        self.queue.dispatch_pending(&mut self.state).map_err(|error| failed(&error))?;
        self.connection.flush().map_err(|error| failed(&error))?;
        if let Some(guard) = self.queue.prepare_read() {
            self.fd.readable().await.map_err(|error| failed(&error))?;
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(failed(&error)),
            }
        }
        self.queue.dispatch_pending(&mut self.state).map_err(|error| failed(&error))?;
        self.apply();
        Ok(())
    }
}

/// The gamma table in a sealed-size memfd, read by the compositor from the start.
fn table(size: usize, kelvin: u32) -> std::io::Result<std::fs::File> {
    let fd = rustix::fs::memfd_create("autark-nightlight", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    let bytes: Vec<u8> = schedule::ramps(size, kelvin).iter().flat_map(|value| value.to_ne_bytes()).collect();
    file.write_all(&bytes)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, handle: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } => match interface.as_str() {
                "wl_output" => {
                    let wl = registry.bind(name, version.min(4), handle, ());
                    state.outputs.push(Output { global: name, wl, control: None, size: None, applied: None, refused: false });
                }
                "zwlr_gamma_control_manager_v1" => state.manager = Some(registry.bind(name, 1, handle, ())),
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(index) = state.outputs.iter().position(|output| output.global == name) {
                    let output = state.outputs.remove(index);
                    if let Some(control) = output.control {
                        control.destroy();
                    }
                    if output.wl.version() >= 3 {
                        output.wl.release();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_gamma_control_v1::ZwlrGammaControlV1, u32> for State {
    fn event(state: &mut Self, control: &zwlr_gamma_control_v1::ZwlrGammaControlV1, event: zwlr_gamma_control_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(output) = state.outputs.iter_mut().find(|output| output.global == *global) else { return };
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => (output.size, output.applied) = (Some(size), None),
            zwlr_gamma_control_v1::Event::Failed => {
                eprintln!("nightlight: an output's gamma can't be changed: another program (wlsunset, gammastep) holds it, or the output has no gamma table");
                control.destroy();
                (output.control, output.size, output.applied, output.refused) = (None, None, None, true);
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(State: zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1);
