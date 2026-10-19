//! Sound through PipeWire, for the bar, Settings and the tools that play effects: the outputs and inputs with
//! their levels and which is the default ([`spawn`]), changes through `wpctl` ([`wpctl`]), the default
//! input's live level ([`meter`]) and the two sound effects ([`play`]), synthesized here rather than shipped
//! as files. PipeWire runs its own loop, so each of these has a thread of its own.
//!
//! `sound.toml` says whether effects play:
//!
//! ```toml
//! effects = true   # the alert chime and the screenshot shutter
//! ```

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use async_channel::Sender;
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::node::{Node, NodeListener};
use pw::properties::properties;
use pw::spa::param::ParamType;
use pw::spa::pod::deserialize::PodDeserializer;
use pw::spa::pod::{Pod, Value, ValueArray};
use pw::types::ObjectType;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level {
    /// As shown to people: the cube root of PipeWire's linear volume, the scale `wpctl` uses.
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: u32,
    pub name: String,
    /// The short name ("Speaker", "HDMI 1"); descriptions repeat the sound card's long name.
    pub nick: String,
    /// What kind of device it is, as shown beside it: "Internal", "Bluetooth", "USB", "HDMI".
    pub kind: &'static str,
    pub level: Option<Level>,
}

/// The outputs and inputs, which of each is the default, and their levels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    pub default_output: Option<u32>,
    pub default_input: Option<u32>,
}

impl Snapshot {
    pub fn output(&self) -> Option<&Device> {
        self.outputs.iter().find(|device| Some(device.id) == self.default_output)
    }

    pub fn input(&self) -> Option<&Device> {
        self.inputs.iter().find(|device| Some(device.id) == self.default_input)
    }
}

/// Changes go through `wpctl`: WirePlumber owns the hardware volume, so a volume set directly on the
/// PipeWire node doesn't stick.
pub fn wpctl(args: &[&str]) {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    std::thread::spawn(move || {
        if let Err(error) = std::process::Command::new("wpctl").args(&args).status() {
            eprintln!("audio: wpctl: {error}");
        }
    });
}

/// Watches PipeWire on a thread of its own; a snapshot comes each time something changes. The channel
/// closes if PipeWire can't be reached.
pub fn spawn() -> async_channel::Receiver<Snapshot> {
    let (updates, received) = async_channel::unbounded();
    std::thread::spawn(move || {
        if let Err(error) = watch(updates) {
            eprintln!("audio: {error}");
        }
    });
    received
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Output,
    Input,
}

struct DeviceState {
    direction: Direction,
    device: Device,
}

#[derive(Default)]
struct State {
    devices: BTreeMap<u32, DeviceState>,
    default_output: Option<String>,
    default_input: Option<String>,
    shown: Option<Snapshot>,
    updates: Option<Sender<Snapshot>>,
}

impl State {
    fn send(&mut self) {
        let list = |direction| self.devices.values().filter(|state| state.direction == direction).map(|state| state.device.clone()).collect::<Vec<_>>();
        let (outputs, inputs) = (list(Direction::Output), list(Direction::Input));
        let find = |devices: &[Device], name: &Option<String>| devices.iter().find(|device| Some(&device.name) == name.as_ref()).map(|device| device.id);
        let snapshot = Snapshot { default_output: find(&outputs, &self.default_output), default_input: find(&inputs, &self.default_input), outputs, inputs };
        if self.shown.as_ref() != Some(&snapshot) {
            if let Some(updates) = &self.updates {
                let _ = updates.send_blocking(snapshot.clone());
            }
            self.shown = Some(snapshot);
        }
    }
}

fn watch(updates: Sender<Snapshot>) -> Result<(), pw::Error> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let registry_weak = registry.downgrade();

    let state = Rc::new(RefCell::new(State { updates: Some(updates), ..State::default() }));
    // Proxies stop reporting once dropped, so they live here with their listeners.
    let nodes: Rc<RefCell<HashMap<u32, (Node, NodeListener)>>> = Rc::default();
    let metadata: Rc<RefCell<Option<(Metadata, MetadataListener)>>> = Rc::default();

    let _registry_listener = registry
        .add_listener_local()
        .global({
            let state = state.clone();
            let nodes = nodes.clone();
            move |global| {
                let Some(registry) = registry_weak.upgrade() else { return };
                let props = global.props;
                match global.type_ {
                    ObjectType::Node => {
                        let direction = match props.and_then(|p| p.get("media.class")) {
                            Some("Audio/Sink") => Direction::Output,
                            Some("Audio/Source") => Direction::Input,
                            _ => return,
                        };
                        let Some(name) = props.and_then(|p| p.get("node.name")) else { return };
                        let nick = props.and_then(|p| p.get("node.nick").or_else(|| p.get("node.description"))).unwrap_or(name);
                        // Refined from the node's full properties once they come.
                        let guess = kind(None, name);
                        let Ok(node) = registry.bind::<Node, _>(global) else { return };
                        let id = global.id;
                        let device = Device { id, name: name.to_owned(), nick: nick.to_owned(), kind: guess, level: None };
                        state.borrow_mut().devices.insert(id, DeviceState { direction, device });
                        state.borrow_mut().send();
                        let listener = node
                            .add_listener_local()
                            .info({
                                let state = state.clone();
                                move |info| {
                                    // Only an update that says so carries the properties.
                                    if !info.change_mask().contains(pw::node::NodeChangeMask::PROPS) {
                                        return;
                                    }
                                    let Some(props) = info.props() else { return };
                                    let bus = props.get("device.bus").or_else(|| props.get("device.api"));
                                    let mut state = state.borrow_mut();
                                    if let Some(device) = state.devices.get_mut(&id) {
                                        device.device.kind = kind(bus, &device.device.name);
                                    }
                                    state.send();
                                }
                            })
                            .param({
                                let state = state.clone();
                                move |_, _, _, _, param| {
                                    let Some(level) = param.and_then(parse_props) else { return };
                                    let mut state = state.borrow_mut();
                                    if let Some(device) = state.devices.get_mut(&id) {
                                        device.device.level = Some(level);
                                    }
                                    state.send();
                                }
                            })
                            .register();
                        node.subscribe_params(&[ParamType::Props]);
                        nodes.borrow_mut().insert(id, (node, listener));
                    }
                    ObjectType::Metadata if props.and_then(|p| p.get("metadata.name")) == Some("default") => {
                        let Ok(bound) = registry.bind::<Metadata, _>(global) else { return };
                        let listener = bound
                            .add_listener_local()
                            .property({
                                let state = state.clone();
                                move |_, key, _, value| {
                                    let mut state = state.borrow_mut();
                                    match key {
                                        Some("default.audio.sink") => state.default_output = value.and_then(node_name),
                                        Some("default.audio.source") => state.default_input = value.and_then(node_name),
                                        _ => return 0,
                                    }
                                    state.send();
                                    0
                                }
                            })
                            .register();
                        *metadata.borrow_mut() = Some((bound, listener));
                    }
                    _ => {}
                }
            }
        })
        .global_remove(move |id| {
            nodes.borrow_mut().remove(&id);
            let mut state = state.borrow_mut();
            if state.devices.remove(&id).is_some() {
                state.send();
            }
        })
        .register();

    main_loop.run();
    Ok(())
}

/// What to call a device's kind, from its bus (or API) when known, else from its name.
fn kind(bus: Option<&str>, name: &str) -> &'static str {
    match bus {
        Some("bluetooth" | "bluez5") => "Bluetooth",
        Some("usb") => "USB",
        _ if name.contains("hdmi") || name.contains("HDMI") => "HDMI",
        Some("pci" | "alsa") => "Internal",
        Some(_) => "",
        None if name.starts_with("bluez_") => "Bluetooth",
        None if name.starts_with("alsa_output.usb-") || name.starts_with("alsa_input.usb-") => "USB",
        None if name.starts_with("alsa_") => "Internal",
        None => "",
    }
}

/// The volume and mute from a node's `Props`; channels can differ, so the loudest one counts.
fn parse_props(param: &Pod) -> Option<Level> {
    let (_, Value::Object(object)) = PodDeserializer::deserialize_any_from(param.as_bytes()).ok()? else { return None };
    let mut level = Level::default();
    let mut found = false;
    for property in object.properties {
        match (property.key, property.value) {
            (pw::spa::sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(volumes))) => {
                level.volume = volumes.into_iter().fold(0.0, f32::max).cbrt();
                found = true;
            }
            (pw::spa::sys::SPA_PROP_mute, Value::Bool(muted)) => level.muted = muted,
            _ => {}
        }
    }
    found.then_some(level)
}

/// `default.audio.sink` holds JSON such as `{ "name": "alsa_output.pci-0000_00_1f.3.analog-stereo" }`.
fn node_name(value: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(value).ok()?;
    Some(json.get("name")?.as_str()?.to_owned())
}

/// The raw-audio format a stream asks for: 32-bit float at `rate` (or the graph's rate when 0), `channels`.
fn format(rate: u32, channels: u32) -> Vec<u8> {
    let mut info = pw::spa::param::audio::AudioInfoRaw::new();
    info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
    if rate > 0 {
        info.set_rate(rate);
    }
    if channels > 0 {
        info.set_channels(channels);
        let mut position = [0; pw::spa::param::audio::MAX_CHANNELS];
        position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_MONO;
        info.set_position(position);
    }
    let object = pw::spa::pod::Object { type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(), id: ParamType::EnumFormat.as_raw(), properties: info.into() };
    pw::spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).map(|(cursor, _)| cursor.into_inner()).unwrap_or_default()
}

/// What's playing on the default output, as chunks of interleaved 48 kHz stereo samples, until this is dropped. It
/// listens to the output's monitor, not the microphone, so only what you could hear is in it.
pub struct Capture {
    pub samples: async_channel::Receiver<Vec<f32>>,
    quit: pw::channel::Sender<()>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.quit.send(());
    }
}

/// Starts listening. If the sound system can't be reached the chunks simply never come, and the error is printed.
pub fn capture_output() -> Capture {
    // Room for several seconds, so a busy reader loses nothing; a reader that stops loses the newest.
    let (chunks, received) = async_channel::bounded(512);
    let (quit, quit_received) = pw::channel::channel();
    std::thread::spawn(move || {
        if let Err(error) = capture(chunks, quit_received) {
            eprintln!("audio: capture: {error}");
        }
    });
    Capture { samples: received, quit }
}

/// 32-bit float stereo at 48 kHz, which the graph converts to whatever the output runs at.
fn stereo_format() -> Vec<u8> {
    let mut info = pw::spa::param::audio::AudioInfoRaw::new();
    info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(2);
    let mut position = [0; pw::spa::param::audio::MAX_CHANNELS];
    position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let object = pw::spa::pod::Object { type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(), id: ParamType::EnumFormat.as_raw(), properties: info.into() };
    pw::spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).map(|(cursor, _)| cursor.into_inner()).unwrap_or_default()
}

fn capture(chunks: Sender<Vec<f32>>, quit: pw::channel::Receiver<()>) -> Result<(), pw::Error> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let _quit = quit.attach(main_loop.loop_(), {
        let main_loop = main_loop.clone();
        move |()| main_loop.quit()
    });
    let stream = pw::stream::StreamBox::new(
        &core,
        "autark-recorder",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
            *pw::keys::STREAM_CAPTURE_SINK => "true",
            *pw::keys::APP_NAME => "Screen recording",
        },
    )?;
    let _listener = stream
        .add_local_listener_with_user_data(chunks)
        .process(|stream, chunks| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let Some(data) = buffer.datas_mut().first_mut() else { return };
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else { return };
            let samples: Vec<f32> = bytes[..size.min(bytes.len())].as_chunks::<4>().0.iter().map(|sample| f32::from_le_bytes(*sample)).collect();
            if !samples.is_empty() {
                let _ = chunks.try_send(samples);
            }
        })
        .register()?;
    let format = stereo_format();
    let mut params = [Pod::from_bytes(&format).ok_or(pw::Error::CreationFailed)?];
    stream.connect(pw::spa::utils::Direction::Input, None, pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)?;
    main_loop.run();
    Ok(())
}

/// The default input's level while this is kept: peaks from 0 to 1, about every 20 ms. Dropping it stops
/// listening, and with it the microphone.
pub struct Meter {
    pub peaks: async_channel::Receiver<f32>,
    quit: pw::channel::Sender<()>,
}

impl Drop for Meter {
    fn drop(&mut self) {
        let _ = self.quit.send(());
    }
}

pub fn meter() -> Meter {
    let (peaks, received) = async_channel::bounded(4);
    let (quit, quit_received) = pw::channel::channel();
    std::thread::spawn(move || {
        if let Err(error) = listen(peaks, quit_received) {
            eprintln!("audio: meter: {error}");
        }
    });
    Meter { peaks: received, quit }
}

fn listen(peaks: Sender<f32>, quit: pw::channel::Receiver<()>) -> Result<(), pw::Error> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let _quit = quit.attach(main_loop.loop_(), {
        let main_loop = main_loop.clone();
        move |()| main_loop.quit()
    });
    let stream = pw::stream::StreamBox::new(
        &core,
        "autark-meter",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "DSP",
            *pw::keys::APP_NAME => "Settings",
        },
    )?;
    let _listener = stream
        .add_local_listener_with_user_data(peaks)
        .process(|stream, peaks| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let Some(data) = buffer.datas_mut().first_mut() else { return };
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else { return };
            let peak = bytes[..size.min(bytes.len())].as_chunks::<4>().0.iter().map(|sample| f32::from_le_bytes(*sample).abs()).fold(0.0, f32::max);
            // Dropped rather than queued when the window is busy: only the latest matters.
            let _ = peaks.try_send(peak.min(1.0));
        })
        .register()?;
    let format = format(0, 1);
    let mut params = [Pod::from_bytes(&format).ok_or(pw::Error::CreationFailed)?];
    stream.connect(pw::spa::utils::Direction::Input, None, pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)?;
    main_loop.run();
    Ok(())
}

/// The sound effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A notification arriving.
    Alert,
    /// A screenshot being taken.
    Shutter,
}

#[derive(Deserialize)]
#[serde(default)]
struct Sound {
    effects: bool,
}

impl Default for Sound {
    fn default() -> Self {
        Self { effects: true }
    }
}

/// The rate the effects are made at.
const RATE: u32 = 48_000;

/// Plays `effect` unless `sound.toml` turns effects off. It returns at once.
pub fn play(effect: Effect) {
    if !crate::config::load::<Sound>("sound").effects {
        return;
    }
    std::thread::spawn(move || {
        // The first buffers can go before the stream is linked, so it starts with 50 ms of quiet.
        let mut padded = vec![0.0; RATE as usize / 20];
        padded.extend(samples(effect, RATE));
        if let Err(error) = sound(padded) {
            eprintln!("audio: effect: {error}");
        }
    });
}

fn sound(samples: Vec<f32>) -> Result<(), pw::Error> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "autark-effect",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Notification",
        },
    )?;
    // How far it has played, and how many quiet buffers have followed the end so it's heard in full.
    let position = (0_usize, 0_u32);
    let _listener = stream
        .add_local_listener_with_user_data(position)
        .process({
            let main_loop = main_loop.clone();
            move |stream, (played, quiet)| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                // Only what the graph asks for is played; anything past it would be dropped.
                let requested = buffer.requested() as usize;
                let Some(data) = buffer.datas_mut().first_mut() else { return };
                let mut frames = 0;
                if let Some(bytes) = data.data() {
                    frames = bytes.len() / 4;
                    if requested > 0 {
                        frames = frames.min(requested);
                    }
                    for (index, sample) in bytes[..frames * 4].as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        *sample = samples.get(*played + index).copied().unwrap_or(0.0).to_le_bytes();
                    }
                }
                *played += frames;
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = 4;
                *chunk.size_mut() = (frames * 4) as u32;
                if *played >= samples.len() {
                    *quiet += 1;
                    if *quiet > 4 {
                        main_loop.quit();
                    }
                }
            }
        })
        .register()?;
    // Given up on if it can't play by then, as when nothing links streams to an output.
    let timer = main_loop.loop_().add_timer({
        let main_loop = main_loop.clone();
        move |_| main_loop.quit()
    });
    let _ = timer.update_timer(Some(std::time::Duration::from_secs(3)), None);
    let format = format(RATE, 1);
    let mut params = [Pod::from_bytes(&format).ok_or(pw::Error::CreationFailed)?];
    stream.connect(pw::spa::utils::Direction::Output, None, pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)?;
    main_loop.run();
    Ok(())
}

/// An effect as mono samples at `rate`.
pub fn samples(effect: Effect, rate: u32) -> Vec<f32> {
    let rate = rate as f32;
    match effect {
        // Two soft notes a fifth apart, each a sine with a little of its octave, fading over a third of a second.
        Effect::Alert => {
            let length = (rate * 0.75) as usize;
            let notes = [(0.0, 880.0), (0.12, 1318.5)];
            (0..length)
                .map(|index| {
                    let time = index as f32 / rate;
                    notes
                        .iter()
                        .filter(|(start, _)| time >= *start)
                        .map(|(start, pitch)| {
                            let since = time - start;
                            let attack = (since / 0.005).min(1.0);
                            let wave = (std::f32::consts::TAU * pitch * since).sin() + 0.2 * (std::f32::consts::TAU * pitch * 2.0 * since).sin();
                            wave * attack * (-since / 0.12).exp() * 0.2
                        })
                        .sum()
                })
                .collect()
        }
        // A shutter's click and clack: two bursts of softened noise 70 ms apart.
        Effect::Shutter => {
            let length = (rate * 0.18) as usize;
            let mut seed: u32 = 0x2545_F491;
            let mut smooth = 0.0;
            (0..length)
                .map(|index| {
                    let time = index as f32 / rate;
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let noise = (seed >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0;
                    smooth += 0.35 * (noise - smooth);
                    let burst = |start: f32, loudness: f32| if time >= start { loudness * (-(time - start) / 0.012).exp() } else { 0.0 };
                    smooth * (burst(0.0, 0.5) + burst(0.07, 0.35))
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_are_short_and_quiet() {
        for effect in [Effect::Alert, Effect::Shutter] {
            let samples = samples(effect, RATE);
            assert!(samples.len() < RATE as usize, "{effect:?} is under a second");
            let peak = samples.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            assert!(peak > 0.05 && peak < 0.6, "{effect:?} peaks at {peak}");
            assert!(samples.last().unwrap().abs() < 0.01, "{effect:?} fades out");
        }
    }

    #[test]
    fn kinds_and_names() {
        assert_eq!(kind(Some("bluetooth"), "bluez_output.x"), "Bluetooth");
        assert_eq!(kind(Some("pci"), "alsa_output.pci-0000_00_1f.3.hdmi-stereo"), "HDMI");
        assert_eq!(kind(Some("pci"), "alsa_output.pci-0000_00_1f.3.analog-stereo"), "Internal");
        assert_eq!(kind(None, "alsa_input.usb-Blue_Yeti-00.analog-stereo"), "USB");
        assert_eq!(kind(None, "bluez_output.AC_80_0A.1"), "Bluetooth");
        assert_eq!(node_name(r#"{ "name": "alsa_input.usb-mic" }"#).as_deref(), Some("alsa_input.usb-mic"));
    }
}
