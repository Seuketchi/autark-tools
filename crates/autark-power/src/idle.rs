//! The compositor's side: `ext-idle-notify` says when no one has touched anything for a while (an app's idle
//! inhibitor, such as a playing video, holds it off), and `wlr-output-power-management` turns the screens
//! off and on again.

use std::os::fd::{AsFd, OwnedFd};
use std::time::Duration;

use async_io::Async;
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{ext_idle_notification_v1, ext_idle_notifier_v1};
use wayland_protocols_wlr::output_power_management::v1::client::{zwlr_output_power_manager_v1, zwlr_output_power_v1};

/// Which timer a notification is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    ScreenOff,
    Sleep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Idle(Timer),
    Back(Timer),
}

struct Output {
    global: u32,
    wl: wl_output::WlOutput,
    power: Option<zwlr_output_power_v1::ZwlrOutputPowerV1>,
}

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    power: Option<zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1>,
    outputs: Vec<Output>,
    events: Vec<Event>,
    /// Whether the screens should be off, for outputs plugged in meanwhile.
    off: bool,
}

pub struct Idle {
    connection: Connection,
    queue: EventQueue<State>,
    handle: QueueHandle<State>,
    state: State,
    fd: Async<OwnedFd>,
    notifications: Vec<ext_idle_notification_v1::ExtIdleNotificationV1>,
}

impl Idle {
    pub fn connect() -> Result<Self, String> {
        let connection = Connection::connect_to_env().map_err(|error| format!("no Wayland display: {error}"))?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|error| format!("Wayland: {error}"))?;
        if state.notifier.is_none() || state.seat.is_none() {
            return Err("the compositor doesn't offer ext-idle-notify (Wayfire 0.9 and later have it)".into());
        }
        if state.power.is_none() {
            eprintln!("power: the compositor doesn't offer wlr-output-power-management, so screens stay on");
        }
        let fd = connection.as_fd().try_clone_to_owned().map_err(|error| format!("Wayland: {error}"))?;
        let fd = Async::new_nonblocking(fd).map_err(|error| format!("Wayland: {error}"))?;
        Ok(Self { connection, queue, handle, state, fd, notifications: Vec::new() })
    }

    /// Starts the timers afresh; none means never.
    pub fn set(&mut self, screen_off: Option<Duration>, sleep: Option<Duration>) {
        for notification in self.notifications.drain(..) {
            notification.destroy();
        }
        let (Some(notifier), Some(seat)) = (&self.state.notifier, &self.state.seat) else { return };
        for (timer, timeout) in [(Timer::ScreenOff, screen_off), (Timer::Sleep, sleep)] {
            if let Some(timeout) = timeout {
                let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
                self.notifications.push(notifier.get_idle_notification(milliseconds, seat, &self.handle, timer));
            }
        }
        let _ = self.connection.flush();
    }

    /// Turns every screen off or back on.
    pub fn screens(&mut self, off: bool) {
        self.state.off = off;
        self.state.apply(&self.handle);
        let _ = self.connection.flush();
    }

    /// Waits for the compositor and returns what happened.
    pub async fn dispatch(&mut self) -> Result<Vec<Event>, String> {
        let failed = |error: &dyn std::fmt::Display| format!("Wayland: {error}");
        self.queue.dispatch_pending(&mut self.state).map_err(|error| failed(&error))?;
        self.connection.flush().map_err(|error| failed(&error))?;
        if self.state.events.is_empty()
            && let Some(guard) = self.queue.prepare_read()
        {
            self.fd.readable().await.map_err(|error| failed(&error))?;
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(failed(&error)),
            }
        }
        self.queue.dispatch_pending(&mut self.state).map_err(|error| failed(&error))?;
        self.state.apply(&self.handle);
        let _ = self.connection.flush();
        Ok(std::mem::take(&mut self.state.events))
    }
}

impl Drop for Idle {
    /// Screens left off would stay off with no one to turn them back on, so whatever ends this (a signal,
    /// an error, a panic) brings them back.
    fn drop(&mut self) {
        if self.state.off {
            self.screens(false);
        }
    }
}

impl State {
    fn apply(&mut self, handle: &QueueHandle<State>) {
        let Some(manager) = &self.power else { return };
        let mode = if self.off { zwlr_output_power_v1::Mode::Off } else { zwlr_output_power_v1::Mode::On };
        for output in &mut self.outputs {
            let power = output.power.get_or_insert_with(|| manager.get_output_power(&output.wl, handle, output.global));
            power.set_mode(mode);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, handle: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } => match interface.as_str() {
                "wl_seat" if state.seat.is_none() => state.seat = Some(registry.bind(name, version.min(1), handle, ())),
                "ext_idle_notifier_v1" => state.notifier = Some(registry.bind(name, 1, handle, ())),
                "zwlr_output_power_manager_v1" => state.power = Some(registry.bind(name, 1, handle, ())),
                "wl_output" => {
                    let wl: wl_output::WlOutput = registry.bind(name, version.min(2), handle, name);
                    state.outputs.push(Output { global: name, wl, power: None });
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => state.outputs.retain(|output| {
                if output.global != name {
                    return true;
                }
                if let Some(power) = &output.power {
                    power.destroy();
                }
                false
            }),
            _ => {}
        }
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, Timer> for State {
    fn event(state: &mut Self, _: &ext_idle_notification_v1::ExtIdleNotificationV1, event: ext_idle_notification_v1::Event, timer: &Timer, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.events.push(Event::Idle(*timer)),
            ext_idle_notification_v1::Event::Resumed => state.events.push(Event::Back(*timer)),
            _ => {}
        }
    }
}

impl Dispatch<zwlr_output_power_v1::ZwlrOutputPowerV1, u32> for State {
    fn event(state: &mut Self, power: &zwlr_output_power_v1::ZwlrOutputPowerV1, event: zwlr_output_power_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        // Failed: the output can't be powered off, or went away; it's asked again next time.
        if let zwlr_output_power_v1::Event::Failed = event {
            power.destroy();
            if let Some(output) = state.outputs.iter_mut().find(|output| output.global == *global && output.power.as_ref().is_some_and(|ours| ours.id() == power.id())) {
                output.power = None;
            }
        }
    }
}

wayland_client::delegate_noop!(State: ignore wl_seat::WlSeat);
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(_: &mut Self, _: &wl_output::WlOutput, _: wl_output::Event, _: &u32, _: &Connection, _: &QueueHandle<Self>) {}
}
wayland_client::delegate_noop!(State: ext_idle_notifier_v1::ExtIdleNotifierV1);
wayland_client::delegate_noop!(State: zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_are_told_apart() {
        assert_ne!(Event::Idle(Timer::ScreenOff), Event::Idle(Timer::Sleep));
    }
}
