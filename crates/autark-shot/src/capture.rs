//! Capturing outputs through wlr-screencopy, on a Wayland connection of its own (Wayfire has the protocol
//! built in). Each output comes back as pixels with its place on the desktop, from xdg-output.

use std::os::unix::fs::FileExt;

use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1};

/// Byte order of a pixel in memory (little-endian `wl_shm` formats read backwards).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pixels {
    Bgrx,
    Bgra,
    Rgbx,
    Rgba,
}

#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Pixels,
    pub data: Vec<u8>,
}

impl Image {
    /// The part of the image inside `(x, y, w, h)`, in pixels, clamped to the image.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Image {
        let x = x.min(self.width);
        let y = y.min(self.height);
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let start = (row * self.stride + x * 4) as usize;
            data.extend_from_slice(&self.data[start..start + (w * 4) as usize]);
        }
        Image { width: w, height: h, stride: w * 4, pixels: self.pixels, data }
    }
}

/// One output's capture.
#[derive(Clone, Debug)]
pub struct Shot {
    /// The connector name (`eDP-1`), as GDK's monitors have it.
    pub name: String,
    /// Place and size on the desktop, in logical pixels.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub image: Image,
}

impl Shot {
    /// Physical pixels per logical one.
    pub fn scale(&self) -> f64 {
        if self.width > 0 { f64::from(self.image.width) / f64::from(self.width) } else { 1.0 }
    }
}

#[derive(Default)]
struct Output {
    wl: Option<wl_output::WlOutput>,
    name: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Default)]
struct Frame {
    buffer: Option<(wl_shm::Format, u32, u32, u32)>,
    /// Version 3 says when every buffer type has been offered; earlier versions offer only one.
    offered: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    shm: Option<wl_shm::WlShm>,
    xdg: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    screencopy: Option<(zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, u32)>,
    outputs: Vec<Output>,
    frame: Frame,
}

/// Every output, captured now. `pointer` puts the pointer in the picture.
pub fn all(pointer: bool) -> Result<Vec<Shot>, String> {
    let connection = Connection::connect_to_env().map_err(|error| format!("no Wayland display: {error}"))?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut state = State::default();
    let roundtrip = |queue: &mut wayland_client::EventQueue<State>, state: &mut State| queue.roundtrip(state).map_err(|error| format!("Wayland: {error}"));
    roundtrip(&mut queue, &mut state)?;
    let shm = state.shm.clone().ok_or("the compositor has no wl_shm")?;
    let (manager, version) = state.screencopy.clone().ok_or("the compositor doesn't offer wlr-screencopy (Wayfire has it built in)")?;
    if let Some(xdg) = &state.xdg {
        for (index, output) in state.outputs.iter().enumerate() {
            if let Some(wl) = &output.wl {
                xdg.get_xdg_output(wl, &handle, index);
            }
        }
    }
    roundtrip(&mut queue, &mut state)?;

    let mut shots = Vec::new();
    for index in 0..state.outputs.len() {
        let Some(wl) = state.outputs[index].wl.clone() else { continue };
        state.frame = Frame::default();
        let frame = manager.capture_output(i32::from(pointer), &wl, &handle, ());
        while !(state.frame.failed || (state.frame.buffer.is_some() && (version < 3 || state.frame.offered))) {
            queue.blocking_dispatch(&mut state).map_err(|error| format!("Wayland: {error}"))?;
        }
        let Some((format, width, height, stride)) = state.frame.buffer.filter(|_| !state.frame.failed) else {
            return Err(format!("capturing {} failed", state.outputs[index].name));
        };
        let pixels = match format {
            wl_shm::Format::Xrgb8888 => Pixels::Bgrx,
            wl_shm::Format::Argb8888 => Pixels::Bgra,
            wl_shm::Format::Xbgr8888 => Pixels::Rgbx,
            wl_shm::Format::Abgr8888 => Pixels::Rgba,
            other => return Err(format!("the compositor offered pixels as {other:?}, which isn't handled")),
        };
        let size = (stride * height) as usize;
        let fd = rustix::fs::memfd_create("autark-shot", rustix::fs::MemfdFlags::CLOEXEC).map_err(|error| format!("memfd: {error}"))?;
        let file = std::fs::File::from(fd);
        file.set_len(size as u64).map_err(|error| format!("memfd: {error}"))?;
        let pool = shm.create_pool(std::os::fd::AsFd::as_fd(&file), size as i32, &handle, ());
        let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, &handle, ());
        frame.copy(&buffer);
        while !(state.frame.ready || state.frame.failed) {
            queue.blocking_dispatch(&mut state).map_err(|error| format!("Wayland: {error}"))?;
        }
        frame.destroy();
        buffer.destroy();
        pool.destroy();
        if state.frame.failed {
            return Err(format!("capturing {} failed", state.outputs[index].name));
        }
        let mut data = vec![0; size];
        file.read_exact_at(&mut data, 0).map_err(|error| format!("memfd: {error}"))?;
        if state.frame.y_invert {
            let rows: Vec<&[u8]> = data.chunks(stride as usize).rev().collect();
            data = rows.concat();
        }
        let output = &state.outputs[index];
        // Without xdg-output, the pixel size is the logical size and outputs sit side by side.
        let (logical_width, logical_height) = if output.width > 0 { (output.width, output.height) } else { (width as i32, height as i32) };
        shots.push(Shot { name: output.name.clone(), x: output.x, y: output.y, width: logical_width, height: logical_height, image: Image { width, height, stride, pixels, data } });
    }
    Ok(shots)
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, handle: &QueueHandle<Self>) {
        let wl_registry::Event::Global { name, interface, version } = event else { return };
        match interface.as_str() {
            "wl_shm" => state.shm = Some(registry.bind(name, 1, handle, ())),
            "wl_output" => {
                let index = state.outputs.len();
                let wl = registry.bind(name, version.min(4), handle, index);
                state.outputs.push(Output { wl: Some(wl), ..Output::default() });
            }
            "zxdg_output_manager_v1" => state.xdg = Some(registry.bind(name, version.min(3), handle, ())),
            "zwlr_screencopy_manager_v1" => {
                let version = version.min(3);
                state.screencopy = Some((registry.bind(name, version, handle, ()), version));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, index: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event
            && let Some(output) = state.outputs.get_mut(*index)
        {
            output.name = name;
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for State {
    fn event(state: &mut Self, _: &zxdg_output_v1::ZxdgOutputV1, event: zxdg_output_v1::Event, index: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let Some(output) = state.outputs.get_mut(*index) else { return };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => (output.x, output.y) = (x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => (output.width, output.height) = (width, height),
            zxdg_output_v1::Event::Name { name } if output.name.is_empty() => output.name = name,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(state: &mut Self, _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, event: zwlr_screencopy_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_screencopy_frame_v1::Event;
        match event {
            Event::Buffer { format: WEnum::Value(format), width, height, stride } => state.frame.buffer = Some((format, width, height, stride)),
            Event::BufferDone => state.frame.offered = true,
            Event::Flags { flags: WEnum::Value(flags) } => state.frame.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert),
            Event::Ready { .. } => state.frame.ready = true,
            Event::Failed => state.frame.failed = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: zxdg_output_manager_v1::ZxdgOutputManagerV1);
delegate_noop!(State: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_takes_rows_and_clamps() {
        // 3 × 2 image, pixel n holds bytes [n; 4].
        let data: Vec<u8> = (0..6u8).flat_map(|n| [n; 4]).collect();
        let image = Image { width: 3, height: 2, stride: 12, pixels: Pixels::Bgrx, data };
        let part = image.crop(1, 0, 2, 2);
        assert_eq!((part.width, part.height, part.stride), (2, 2, 8));
        assert_eq!(part.data, [1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 5, 5, 5, 5]);
        assert_eq!(image.crop(2, 1, 10, 10).data, [5, 5, 5, 5]);
    }
}
