//! Window outlines for picking a window, from Wayfire's IPC socket (`$WAYFIRE_SOCKET`, plugins `ipc` and
//! `ipc-rules`). Without it, window mode isn't offered.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde::Deserialize;

/// A window's outline on the desktop, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Deserialize)]
struct View {
    #[serde(default)]
    title: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    minimized: bool,
    geometry: Geometry,
    #[serde(rename = "output-id", default)]
    output: i64,
    #[serde(rename = "last-focus-timestamp", default)]
    focused_at: i64,
}

#[derive(Deserialize)]
struct Geometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Deserialize)]
struct Output {
    id: i64,
    geometry: Geometry,
}

/// Open windows, most recently focused first; empty when the IPC socket isn't there.
pub fn list() -> Vec<Window> {
    request("window-rules/list-views")
        .and_then(|views| {
            let views: Vec<View> = serde_json::from_value(views).ok()?;
            let outputs: Vec<Output> = request("window-rules/list-outputs").and_then(|outputs| serde_json::from_value(outputs).ok()).unwrap_or_default();
            Some(parse(views, &outputs))
        })
        .unwrap_or_default()
}

fn parse(mut views: Vec<View>, outputs: &[Output]) -> Vec<Window> {
    views.retain(|view| view.role == "toplevel" && view.mapped && !view.minimized && view.geometry.width > 0);
    views.sort_by_key(|view| std::cmp::Reverse(view.focused_at));
    views
        .into_iter()
        .map(|view| {
            // A view's geometry is relative to its output.
            let (ox, oy) = outputs.iter().find(|output| output.id == view.output).map_or((0, 0), |output| (output.geometry.x, output.geometry.y));
            Window { title: view.title, x: view.geometry.x + ox, y: view.geometry.y + oy, width: view.geometry.width, height: view.geometry.height }
        })
        .collect()
}

/// One IPC call: a little-endian length, then JSON each way.
fn request(method: &str) -> Option<serde_json::Value> {
    let path = std::env::var_os("WAYFIRE_SOCKET")?;
    let mut stream = UnixStream::connect(path).ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_mapped_toplevels_in_focus_order() {
        let views: Vec<View> = serde_json::from_str(
            r#"[{"title":"old","role":"toplevel","mapped":true,"geometry":{"x":10,"y":20,"width":300,"height":200},"output-id":2,"last-focus-timestamp":5},
                {"title":"panel","role":"desktop-environment","mapped":true,"geometry":{"x":0,"y":0,"width":1440,"height":32},"output-id":1},
                {"title":"new","role":"toplevel","mapped":true,"geometry":{"x":0,"y":0,"width":100,"height":100},"output-id":1,"last-focus-timestamp":9},
                {"title":"hidden","role":"toplevel","mapped":true,"minimized":true,"geometry":{"x":0,"y":0,"width":100,"height":100},"output-id":1}]"#,
        )
        .unwrap();
        let outputs: Vec<Output> = serde_json::from_str(r#"[{"id":1,"geometry":{"x":0,"y":0,"width":1440,"height":900}},{"id":2,"geometry":{"x":1440,"y":0,"width":1920,"height":1080}}]"#).unwrap();
        let windows = parse(views, &outputs);
        assert_eq!(windows.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(), ["new", "old"]);
        assert_eq!((windows[1].x, windows[1].y), (1450, 20));
    }
}
