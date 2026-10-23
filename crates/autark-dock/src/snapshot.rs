//! Pictures of windows. Wayfire can't capture a single window, so this captures the screen and cuts the focused window
//! out of it, using the window's place from Wayfire's IPC socket (plugins `ipc` and `ipc-rules`). It only ever sees a
//! window while it's focused, in front and uncovered, so the picture is how it looked a little after it took focus.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use autark_common::capture::{self, Image, Pixels};
use gtk::prelude::*;
use gtk::{gdk, glib};
use serde::Deserialize;

/// The widest a stored picture gets, in pixels: the list shows it small, so this keeps it sharp on a 2x screen.
const MAX_WIDTH: u32 = 400;

#[derive(Deserialize)]
struct View {
    #[serde(default)]
    role: String,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    minimized: bool,
    #[serde(default)]
    activated: bool,
    #[serde(rename = "output-id", default)]
    output: i64,
    geometry: Geometry,
}

#[derive(Deserialize)]
struct Geometry {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Deserialize)]
struct Output {
    id: i64,
    geometry: Geometry,
}

/// The focused window as it looks now, or `None` when there's no IPC socket, no focused window or the capture fails.
pub fn focused() -> Option<gdk::Texture> {
    let views: Vec<View> = serde_json::from_value(request("window-rules/list-views")?).ok()?;
    let view = views.into_iter().find(|view| view.activated && view.role == "toplevel" && view.mapped && !view.minimized && view.geometry.width > 0.0)?;
    let outputs: Vec<Output> = serde_json::from_value(request("window-rules/list-outputs")?).ok()?;
    // A view's geometry is relative to its output.
    let (ox, oy) = outputs.iter().find(|output| output.id == view.output).map_or((0.0, 0.0), |output| (output.geometry.x, output.geometry.y));
    let (x, y) = (view.geometry.x + ox, view.geometry.y + oy);
    let (centre_x, centre_y) = (x + view.geometry.width / 2.0, y + view.geometry.height / 2.0);

    let shots = capture::all(false).ok()?;
    let shot = shots.iter().find(|shot| {
        (f64::from(shot.x)..f64::from(shot.x + shot.width)).contains(&centre_x) && (f64::from(shot.y)..f64::from(shot.y + shot.height)).contains(&centre_y)
    })?;
    let scale = shot.scale();
    let left = ((x - f64::from(shot.x)) * scale).max(0.0).round() as u32;
    let top = ((y - f64::from(shot.y)) * scale).max(0.0).round() as u32;
    let (width, height) = ((view.geometry.width * scale).round() as u32, (view.geometry.height * scale).round() as u32);
    let image = shot.image.crop(left, top, width, height);
    if image.width == 0 || image.height == 0 {
        return None;
    }
    Some(texture(&shrink(&image)))
}

/// The image with every block of `factor` by `factor` pixels averaged into one, so a big window doesn't cost a big texture.
fn shrink(image: &Image) -> Image {
    let factor = image.width.div_ceil(MAX_WIDTH).max(1);
    if factor == 1 {
        return image.clone();
    }
    let (width, height) = (image.width / factor, image.height / factor);
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        for column in 0..width {
            let mut sum = [0u32; 4];
            for dy in 0..factor {
                let start = ((row * factor + dy) * image.stride + column * factor * 4) as usize;
                for dx in 0..factor as usize {
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(image.data[start + dx * 4 + channel]);
                    }
                }
            }
            data.extend(sum.iter().map(|total| (total / (factor * factor)) as u8));
        }
    }
    Image { width, height, stride: width * 4, pixels: image.pixels, data }
}

fn texture(image: &Image) -> gdk::Texture {
    let format = match image.pixels {
        Pixels::Bgrx => gdk::MemoryFormat::B8g8r8x8,
        Pixels::Bgra => gdk::MemoryFormat::B8g8r8a8Premultiplied,
        Pixels::Rgbx => gdk::MemoryFormat::R8g8b8x8,
        Pixels::Rgba => gdk::MemoryFormat::R8g8b8a8Premultiplied,
    };
    gdk::MemoryTexture::new(image.width as i32, image.height as i32, format, &glib::Bytes::from(&image.data[..]), image.stride as usize).upcast()
}

fn socket() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WAYFIRE_SOCKET") {
        return Some(path.into());
    }
    // Wayfire exports it to what it starts; anything else finds it where Wayfire puts it.
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(PathBuf::from(runtime).join(format!("wayfire-{display}-.socket")))
}

/// One IPC call: a little-endian length, then JSON each way.
fn request(method: &str) -> Option<serde_json::Value> {
    let mut stream = UnixStream::connect(socket()?).ok()?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).ok()?;
    let body = serde_json::json!({ "method": method, "data": {} }).to_string();
    stream.write_all(&(body.len() as u32).to_le_bytes()).ok()?;
    stream.write_all(body.as_bytes()).ok()?;
    let mut length = [0; 4];
    stream.read_exact(&mut length).ok()?;
    let mut reply = vec![0; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut reply).ok()?;
    serde_json::from_slice(&reply).ok()
}
