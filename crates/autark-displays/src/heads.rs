//! The monitors through `wlr-output-management`: what's connected with its modes and state, and applying a
//! whole arrangement at once.

use std::os::fd::{AsFd, OwnedFd};

use async_io::Async;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols_wlr::output_management::v1::client::{zwlr_output_configuration_head_v1, zwlr_output_configuration_v1, zwlr_output_head_v1, zwlr_output_manager_v1, zwlr_output_mode_v1};

use crate::layout::{Head, Mode, Setting};

struct HeadState {
    proxy: zwlr_output_head_v1::ZwlrOutputHeadV1,
    head: Head,
    modes: Vec<(zwlr_output_mode_v1::ZwlrOutputModeV1, Mode)>,
    current: Option<zwlr_output_mode_v1::ZwlrOutputModeV1>,
}

#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Default)]
struct State {
    manager: Option<zwlr_output_manager_v1::ZwlrOutputManagerV1>,
    heads: Vec<HeadState>,
    serial: u32,
    /// A `done` arrived since the heads were last looked at.
    changed: bool,
    outcome: Option<Outcome>,
}

pub struct Outputs {
    connection: Connection,
    queue: EventQueue<State>,
    handle: QueueHandle<State>,
    state: State,
    fd: Async<OwnedFd>,
}

impl Outputs {
    pub fn connect() -> Result<Self, String> {
        let connection = Connection::connect_to_env().map_err(|error| format!("no Wayland display: {error}"))?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
        if state.manager.is_none() {
            return Err("the compositor doesn't offer wlr-output-management (Wayfire and other wlroots compositors have it)".into());
        }
        queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
        let fd = connection.as_fd().try_clone_to_owned().map_err(|error| format!("Wayland: {error}"))?;
        let fd = Async::new_nonblocking(fd).map_err(|error| format!("Wayland: {error}"))?;
        Ok(Self { connection, queue, handle, state, fd })
    }

    /// The monitors as last reported, in the compositor's order.
    pub fn heads(&self) -> Vec<Head> {
        self.state
            .heads
            .iter()
            .map(|state| {
                let mut head = state.head.clone();
                head.modes = state.modes.iter().map(|(_, mode)| mode.clone()).collect();
                head.mode = state.current.as_ref().and_then(|current| state.modes.iter().find(|(proxy, _)| proxy == current)).map(|(_, mode)| Mode { preferred: false, ..mode.clone() });
                head
            })
            .collect()
    }

    /// Whether the compositor has reported a change since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.state.changed)
    }

    /// Applies every setting at once: `Ok(false)` when the compositor cancelled it because the monitors changed
    /// meanwhile (its next report says how they are now).
    pub async fn apply(&mut self, settings: &[Setting]) -> Result<bool, String> {
        let manager = self.state.manager.clone().ok_or("no output manager")?;
        let configuration = manager.create_configuration(self.state.serial, &self.handle, ());
        for setting in settings {
            let Some(state) = self.state.heads.iter().find(|state| state.head.name == setting.name) else { continue };
            match &setting.on {
                None => configuration.disable_head(&state.proxy),
                Some((mode, (x, y), scale, transform)) => {
                    let head = configuration.enable_head(&state.proxy, &self.handle, ());
                    match state.modes.iter().find(|(_, offered)| offered.width == mode.width && offered.height == mode.height && offered.refresh == mode.refresh) {
                        Some((proxy, _)) => head.set_mode(proxy),
                        None => head.set_custom_mode(mode.width, mode.height, mode.refresh),
                    }
                    head.set_position(*x, *y);
                    head.set_scale(*scale);
                    head.set_transform(transform_from(transform));
                }
            }
        }
        configuration.apply();
        self.state.outcome = None;
        while self.state.outcome.is_none() {
            self.dispatch().await?;
        }
        configuration.destroy();
        match self.state.outcome {
            Some(Outcome::Succeeded) => Ok(true),
            Some(Outcome::Failed) => Err("the compositor refused that arrangement".into()),
            _ => Ok(false),
        }
    }

    /// Waits for the compositor to say something and takes it in.
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
        Ok(())
    }
}

fn transform_name(transform: wl_output::Transform) -> &'static str {
    use wl_output::Transform::{self as T, *};
    match transform {
        T::_90 => "90",
        T::_180 => "180",
        T::_270 => "270",
        Flipped => "flipped",
        Flipped90 => "flipped-90",
        Flipped180 => "flipped-180",
        Flipped270 => "flipped-270",
        _ => "normal",
    }
}

fn transform_from(name: &str) -> wl_output::Transform {
    use wl_output::Transform::{self as T, *};
    match name {
        "90" => T::_90,
        "180" => T::_180,
        "270" => T::_270,
        "flipped" => Flipped,
        "flipped-90" => Flipped90,
        "flipped-180" => Flipped180,
        "flipped-270" => Flipped270,
        _ => Normal,
    }
}

pub const TRANSFORMS: [&str; 8] = ["normal", "90", "180", "270", "flipped", "flipped-90", "flipped-180", "flipped-270"];

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, handle: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event
            && interface == "zwlr_output_manager_v1"
        {
            state.manager = Some(registry.bind(name, version.min(4), handle, ()));
        }
    }
}

impl Dispatch<zwlr_output_manager_v1::ZwlrOutputManagerV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_output_manager_v1::ZwlrOutputManagerV1, event: zwlr_output_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.heads.push(HeadState { proxy: head, head: Head { scale: 1.0, transform: "normal".into(), ..Head::default() }, modes: Vec::new(), current: None })
            }
            zwlr_output_manager_v1::Event::Done { serial } => (state.serial, state.changed) = (serial, true),
            _ => {}
        }
    }

    event_created_child!(State, zwlr_output_manager_v1::ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (zwlr_output_head_v1::ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<zwlr_output_head_v1::ZwlrOutputHeadV1, ()> for State {
    fn event(state: &mut Self, proxy: &zwlr_output_head_v1::ZwlrOutputHeadV1, event: zwlr_output_head_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_output_head_v1::Event;
        let Some(index) = state.heads.iter().position(|head| head.proxy == *proxy) else { return };
        let entry = &mut state.heads[index];
        match event {
            Event::Name { name } => entry.head.name = name,
            Event::Description { description } => entry.head.description = description,
            Event::PhysicalSize { width, height } => entry.head.physical = (width, height),
            Event::Mode { mode } => entry.modes.push((mode, Mode::default())),
            Event::Enabled { enabled } => {
                entry.head.enabled = enabled != 0;
                if !entry.head.enabled {
                    entry.current = None;
                }
            }
            Event::CurrentMode { mode } => entry.current = Some(mode),
            Event::Position { x, y } => (entry.head.x, entry.head.y) = (x, y),
            Event::Transform { transform: WEnum::Value(transform) } => entry.head.transform = transform_name(transform).into(),
            Event::Scale { scale } => entry.head.scale = scale,
            Event::Make { make } => entry.head.make = make,
            Event::Model { model } => entry.head.model = model,
            Event::SerialNumber { serial_number } => entry.head.serial = serial_number,
            Event::Finished => {
                let gone = state.heads.remove(index);
                if gone.proxy.version() >= 3 {
                    gone.proxy.release();
                }
            }
            _ => {}
        }
    }

    event_created_child!(State, zwlr_output_head_v1::ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (zwlr_output_mode_v1::ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<zwlr_output_mode_v1::ZwlrOutputModeV1, ()> for State {
    fn event(state: &mut Self, proxy: &zwlr_output_mode_v1::ZwlrOutputModeV1, event: zwlr_output_mode_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_output_mode_v1::Event;
        for head in &mut state.heads {
            let Some(position) = head.modes.iter().position(|(mode, _)| mode == proxy) else { continue };
            match event {
                Event::Size { width, height } => (head.modes[position].1.width, head.modes[position].1.height) = (width, height),
                Event::Refresh { refresh } => head.modes[position].1.refresh = refresh,
                Event::Preferred => head.modes[position].1.preferred = true,
                Event::Finished => {
                    let (gone, _) = head.modes.remove(position);
                    if head.current.as_ref() == Some(&gone) {
                        head.current = None;
                    }
                    if gone.version() >= 3 {
                        gone.release();
                    }
                }
                _ => {}
            }
            return;
        }
    }
}

impl Dispatch<zwlr_output_configuration_v1::ZwlrOutputConfigurationV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_output_configuration_v1::ZwlrOutputConfigurationV1, event: zwlr_output_configuration_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.outcome = Some(match event {
            zwlr_output_configuration_v1::Event::Succeeded => Outcome::Succeeded,
            zwlr_output_configuration_v1::Event::Failed => Outcome::Failed,
            _ => Outcome::Cancelled,
        });
    }
}

wayland_client::delegate_noop!(State: ignore zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1);
