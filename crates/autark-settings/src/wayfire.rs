//! Wayfire's own options (keyboard, shortcuts): set live over its IPC socket and written to `wayfire.ini`, so
//! they hold after a restart. The file is edited line by line; comments and order stay as they were.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde_json::Value;

pub use autark_common::ini::{Ini, path as ini_path};

/// One call on `$WAYFIRE_SOCKET` (the `ipc` plugin): a little-endian length, then JSON, each way.
pub fn call(method: &str, data: Value) -> Result<Value, String> {
    let socket = std::env::var_os("WAYFIRE_SOCKET").ok_or("Wayfire's IPC isn't on (add ipc to its plugins)")?;
    let mut stream = UnixStream::connect(&socket).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let message = serde_json::to_vec(&serde_json::json!({ "method": method, "data": data })).map_err(|error| error.to_string())?;
    stream.write_all(&(message.len() as u32).to_le_bytes()).and_then(|_| stream.write_all(&message)).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let mut length = [0; 4];
    stream.read_exact(&mut length).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let mut reply = vec![0; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut reply).map_err(|error| format!("Wayfire IPC: {error}"))?;
    let reply: Value = serde_json::from_slice(&reply).map_err(|error| format!("Wayfire IPC: {error}"))?;
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(format!("Wayfire: {error}")),
        None => Ok(reply),
    }
}

/// An option's current value as Wayfire has it, `section/name`.
pub fn get(option: &str) -> Option<String> {
    let reply = call("wayfire/get-config-option", serde_json::json!({ "option": option })).ok()?;
    match reply.get("value")? {
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// Sets options in `wayfire.ini` and live. Wayfire rereads the file when it changes, so the live call is only
/// for speed and its failure isn't one.
pub fn set(options: &[(&str, String)]) -> Result<(), String> {
    write(options)?;
    let live: serde_json::Map<String, Value> = options.iter().map(|(option, value)| (option.to_string(), Value::String(value.clone()))).collect();
    let _ = call("wayfire/set-config-options", Value::Object(live));
    Ok(())
}

/// Writes options, `section/name`, to `wayfire.ini`.
pub fn write(options: &[(&str, String)]) -> Result<(), String> {
    let options: Vec<(&str, Option<&str>)> = options.iter().map(|(option, value)| (*option, Some(value.as_str()))).collect();
    autark_common::ini::update(&ini_path(), &options)
}
