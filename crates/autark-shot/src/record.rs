//! Screen recording: the screen, or an area of it, captured through wlr-screencopy about 30 times a second,
//! encoded as VP8 and written as WebM to `~/Videos/Screen Recordings`. It runs until `autark-shot record` is
//! run again, which asks it to stop through a file in the runtime folder. What's playing on the output is recorded
//! with it as Opus, unless `record_audio` is off in shot.toml.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use autark_common::capture::{Layout, Pixels, Session};
use crate::opus;
use crate::vpx::{Encoder, Yuv};
use crate::webm::Writer;

const FPS: u32 = 30;

/// A rectangle in the output's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn runtime() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("autark")
}

fn pid_file() -> PathBuf {
    runtime().join("record.pid")
}

fn stop_file() -> PathBuf {
    runtime().join("record.stop")
}

/// Whether a recording is going on: the process that wrote the pid file is still an `autark-shot`.
pub fn running() -> bool {
    let Some(pid) = std::fs::read_to_string(pid_file()).ok().and_then(|text| text.trim().parse::<u32>().ok()) else { return false };
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|name| name.trim() == "autark-shot")
}

/// Asks the recording to finish.
pub fn stop() -> std::io::Result<()> {
    std::fs::create_dir_all(runtime())?;
    std::fs::write(stop_file(), b"")
}

/// `~/Videos/Screen Recordings`.
fn folder() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Videos/Screen Recordings")
}

/// The part of the output to record in pixels, even in both directions as the encoder needs, kept inside it.
/// `area` is logical (x, y, width, height); without it, the whole output.
pub fn region(layout: &Layout, logical: (i32, i32), area: Option<(i32, i32, i32, i32)>) -> Region {
    let scale = if logical.0 > 0 { f64::from(layout.width) / f64::from(logical.0) } else { 1.0 };
    let (x, y, w, h) = match area {
        Some((x, y, w, h)) => {
            let px = |value: i32| (f64::from(value.max(0)) * scale).round() as u32;
            (px(x), px(y), px(w), px(h))
        }
        None => (0, 0, layout.width, layout.height),
    };
    let x = (x & !1).min(layout.width.saturating_sub(2));
    let y = (y & !1).min(layout.height.saturating_sub(2));
    Region { x, y, width: (w.min(layout.width - x) & !1).max(2), height: (h.min(layout.height - y) & !1).max(2) }
}

/// Converts `region` of a captured frame to planar YUV (BT.601), the chroma averaged over each 2 × 2 block.
pub fn to_yuv(data: &[u8], layout: &Layout, region: Region, out: &mut Yuv) {
    let (r, g, b) = match layout.pixels {
        Pixels::Bgrx | Pixels::Bgra => (2, 1, 0),
        Pixels::Rgbx | Pixels::Rgba => (0, 1, 2),
    };
    let row = |index: u32| {
        let source = if layout.y_invert { layout.height - 1 - index } else { index };
        let start = (source * layout.stride + region.x * 4) as usize;
        &data[start..start + (region.width * 4) as usize]
    };
    let width = region.width as usize;
    for pair in 0..region.height / 2 {
        let top = row(region.y + pair * 2);
        let bottom = row(region.y + pair * 2 + 1);
        for column in 0..width / 2 {
            let (mut sum_r, mut sum_g, mut sum_b) = (0i32, 0i32, 0i32);
            for (line, pixels) in [top, bottom].into_iter().enumerate() {
                for dx in 0..2 {
                    let offset = (column * 2 + dx) * 4;
                    let (pr, pg, pb) = (i32::from(pixels[offset + r]), i32::from(pixels[offset + g]), i32::from(pixels[offset + b]));
                    let luma = ((66 * pr + 129 * pg + 25 * pb + 128) >> 8) + 16;
                    out.y[(pair as usize * 2 + line) * width + column * 2 + dx] = luma as u8;
                    sum_r += pr;
                    sum_g += pg;
                    sum_b += pb;
                }
            }
            let (ar, ag, ab) = (sum_r / 4, sum_g / 4, sum_b / 4);
            out.u[pair as usize * (width / 2) + column] = (((-38 * ar - 74 * ag + 112 * ab + 128) >> 8) + 128).clamp(0, 255) as u8;
            out.v[pair as usize * (width / 2) + column] = (((112 * ar - 94 * ag - 18 * ab + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
}

/// Records until asked to stop, and says where the file is. `area` is logical pixels on `output`.
pub fn run(output: Option<&str>, area: Option<(i32, i32, i32, i32)>, pointer: bool, audio: bool) -> Result<PathBuf, String> {
    let mut session = Session::open(output)?;
    let mut data = Vec::new();
    let layout = session.grab(pointer, &mut data)?;
    let region = region(&layout, session.logical, area);

    std::fs::create_dir_all(runtime()).map_err(|error| format!("{}: {error}", runtime().display()))?;
    let _ = std::fs::remove_file(stop_file());
    std::fs::write(pid_file(), std::process::id().to_string()).map_err(|error| format!("{}: {error}", pid_file().display()))?;
    let result = record(&mut session, &mut data, layout, region, pointer, audio);
    let _ = std::fs::remove_file(pid_file());
    let _ = std::fs::remove_file(stop_file());
    result
}

fn record(session: &mut Session, data: &mut Vec<u8>, mut layout: Layout, region: Region, pointer: bool, audio: bool) -> Result<PathBuf, String> {
    let folder = folder();
    std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let now = glib::DateTime::now_local().map_err(|error| error.to_string())?;
    let name = format!("Screen Recording {}.webm", now.format("%Y-%m-%d at %H.%M.%S").map(|text| text.to_string()).unwrap_or_default());
    let path = autark_common::files::free_name(&folder, &name);

    // About 0.07 bits a pixel: sharp text on 1080p at 4 Mbit/s.
    let kbps = (u64::from(region.width) * u64::from(region.height) * u64::from(FPS) * 7 / 100_000).clamp(1_500, 20_000) as u32;
    let mut encoder = Encoder::new(region.width, region.height, FPS, kbps)?;
    let mut sound = if audio { Sound::start() } else { None };
    let head = sound.as_ref().map(|sound| (sound.encoder.head(), sound.encoder.pre_skip));
    let mut writer = Writer::create(&path, region.width, region.height, head.as_ref().map(|(head, skip)| (head.as_slice(), *skip))).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut yuv = Yuv::new(region.width, region.height);
    let frame = Duration::from_millis(u64::from(1000 / FPS));
    let start = Instant::now();
    let mut next = start;
    let mut first = true;
    let mut failure: Option<String> = None;
    notify("Recording the screen", "Run the recording shortcut again to stop.");
    while !stop_file().exists() {
        let mut write_error = None;
        let pts = start.elapsed().as_millis() as i64;
        to_yuv(data, &layout, region, &mut yuv);
        let encoded = encoder.encode(&yuv, pts, first, |packet, key, pts| {
            if let Err(error) = writer.frame(pts, key, packet) {
                write_error = Some(error.to_string());
            }
        });
        first = false;
        if let Some(sound) = sound.as_mut() {
            if let Err(error) = sound.drain(start.elapsed().as_millis() as i64, false, &mut writer) {
                write_error = write_error.or(Some(error));
            }
        }
        if let Some(error) = write_error.or(encoded.err()) {
            failure = Some(error);
            break;
        }
        next += frame;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Behind: don't try to catch up with a burst.
            next = now;
        }
        match session.grab(pointer, data) {
            Ok(grabbed) => layout = grabbed,
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
        if region.x + region.width > layout.width || region.y + region.height > layout.height {
            failure = Some("the screen changed size".to_owned());
            break;
        }
    }
    let mut write_error = None;
    if let Some(sound) = sound.as_mut() {
        if let Err(error) = sound.drain(start.elapsed().as_millis() as i64, true, &mut writer) {
            write_error = Some(error);
        }
    }
    let flushed = encoder.flush(|packet, key, pts| {
        if let Err(error) = writer.frame(pts, key, packet) {
            write_error = Some(error.to_string());
        }
    });
    let finished = writer.finish(i64::from(1000 / FPS)).map_err(|error| format!("{}: {error}", path.display()));
    let outcome = failure.map_or(Ok(()), Err).and(flushed).and(write_error.map_or(Ok(()), Err)).and(finished);
    match outcome {
        Ok(()) => {
            notify("Screen recording saved", &display(&path));
            Ok(path)
        }
        Err(error) => {
            notify("Screen recording stopped", &error);
            Err(error)
        }
    }
}

/// What's playing on the output, encoded as it arrives and kept in step with the video's clock.
struct Sound {
    capture: autark_common::audio::Capture,
    encoder: opus::Encoder,
    /// Samples received and not yet a whole frame.
    pending: Vec<f32>,
    /// Where on the recording's clock the first frame lies, once the first samples arrive.
    origin_ms: Option<i64>,
    frames: i64,
}

/// A frame is 20 ms.
const FRAME_MS: i64 = 20;

impl Sound {
    /// Starts listening; without it the recording is silent, and says why.
    fn start() -> Option<Self> {
        match opus::Encoder::new(128_000) {
            Ok(encoder) => Some(Self { capture: autark_common::audio::capture_output(), encoder, pending: Vec::new(), origin_ms: None, frames: 0 }),
            Err(error) => {
                eprintln!("autark-shot: no sound in this recording: {error}");
                None
            }
        }
    }

    /// Encodes what has arrived. While nothing plays the output may send nothing at all, so silence fills the gap
    /// and the sound stays where it belongs against the picture. `last` pads the partial frame at the end.
    fn drain(&mut self, now_ms: i64, last: bool, writer: &mut Writer) -> Result<(), String> {
        let mut arrived = 0;
        while let Ok(chunk) = self.capture.samples.try_recv() {
            arrived += chunk.len();
            self.pending.extend(chunk);
        }
        let whole = opus::FRAME * opus::CHANNELS;
        // The samples that just arrived were played a moment ago, so the first frame starts that much earlier.
        let origin = *self.origin_ms.get_or_insert_with(|| (now_ms - (arrived / opus::CHANNELS) as i64 * 1000 / i64::from(opus::RATE)).max(0));
        loop {
            let silent = self.pending.len() < whole && now_ms - (origin + self.frames * FRAME_MS) > 200;
            if self.pending.len() >= whole || silent || (last && !self.pending.is_empty()) {
                self.pending.resize(whole.max(self.pending.len()), 0.0);
                let frame: Vec<f32> = self.pending.drain(..whole).collect();
                let packet = self.encoder.encode(&frame)?;
                writer.sound(origin + self.frames * FRAME_MS, &packet).map_err(|error| error.to_string())?;
                self.frames += 1;
            } else {
                return Ok(());
            }
        }
    }
}

/// `~/Videos/…` for a path in the home folder.
fn display(path: &Path) -> String {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    path.strip_prefix(&home).map_or_else(|_| path.display().to_string(), |rest| format!("~/{}", rest.display()))
}

/// A notification through `org.freedesktop.Notifications`; a recording shouldn't fail for want of one.
fn notify(summary: &str, body: &str) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return };
    let args = ("autark-shot", 0u32, "autark-shot", summary, body, Vec::<&str>::new(), std::collections::HashMap::<&str, glib::Variant>::new(), -1i32).to_variant();
    let _ = bus.call_sync(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(width: u32, height: u32) -> Layout {
        Layout { width, height, stride: width * 4, pixels: Pixels::Bgrx, y_invert: false }
    }

    #[test]
    fn regions_are_even_and_inside_the_output() {
        let screen = layout(1921, 1081);
        assert_eq!(region(&screen, (1921, 1081), None), Region { x: 0, y: 0, width: 1920, height: 1080 });
        // A logical area on a 2x output, with odd edges.
        let two = layout(2000, 1000);
        assert_eq!(region(&two, (1000, 500), Some((11, 21, 101, 51))), Region { x: 22, y: 42, width: 202, height: 102 });
        // Hanging off the edge is cut back.
        assert_eq!(region(&two, (1000, 500), Some((990, 490, 100, 100))), Region { x: 1980, y: 980, width: 20, height: 20 });
    }

    #[test]
    fn black_and_white_and_red_convert_to_known_values() {
        // 2 × 2: black, white, red, white (B, G, R, unused).
        let data: Vec<u8> = [[0, 0, 0, 0], [255, 255, 255, 0], [0, 0, 255, 0], [255, 255, 255, 0]].concat();
        let screen = layout(2, 2);
        let mut yuv = Yuv::new(2, 2);
        to_yuv(&data, &screen, Region { x: 0, y: 0, width: 2, height: 2 }, &mut yuv);
        assert_eq!(yuv.y[0], 16);
        assert_eq!(yuv.y[1], 235);
        assert!((i32::from(yuv.y[2]) - 82).abs() <= 1, "red luma {}", yuv.y[2]);
        assert_eq!(yuv.u.len(), 1);
    }

    #[test]
    fn inverted_frames_are_read_from_the_bottom() {
        // Row 0 is white and row 1 black in memory, so right way up the top row is black.
        let data: Vec<u8> = [[255, 255, 255, 0]; 2].into_iter().chain([[0, 0, 0, 0]; 2]).flatten().collect();
        let screen = Layout { y_invert: true, ..layout(2, 2) };
        let mut yuv = Yuv::new(2, 2);
        to_yuv(&data, &screen, Region { x: 0, y: 0, width: 2, height: 2 }, &mut yuv);
        assert_eq!((yuv.y[0], yuv.y[2]), (16, 235));
    }
}
