//! One session and one system bus connection for every pane.

use std::cell::RefCell;

thread_local! {
    static SESSION: RefCell<Option<zbus::Connection>> = const { RefCell::new(None) };
    static SYSTEM: RefCell<Option<zbus::Connection>> = const { RefCell::new(None) };
}

pub async fn session() -> zbus::Result<zbus::Connection> {
    if let Some(connection) = SESSION.with_borrow(Clone::clone) {
        return Ok(connection);
    }
    let connection = zbus::Connection::session().await?;
    SESSION.set(Some(connection.clone()));
    Ok(connection)
}

pub async fn system() -> zbus::Result<zbus::Connection> {
    if let Some(connection) = SYSTEM.with_borrow(Clone::clone) {
        return Ok(connection);
    }
    let connection = zbus::Connection::system().await?;
    SYSTEM.set(Some(connection.clone()));
    Ok(connection)
}

/// An error as a sentence, naming the daemon when it isn't running.
pub fn message(error: zbus::Error, daemon: &str) -> String {
    // Unknown, or D-Bus couldn't start it.
    let absent = |name: &str| name == "org.freedesktop.DBus.Error.ServiceUnknown" || name.starts_with("org.freedesktop.DBus.Error.Spawn.");
    match error {
        zbus::Error::MethodError(name, ..) if absent(name.as_str()) => format!("{daemon} isn't running."),
        zbus::Error::FDO(error) if absent(zbus::DBusError::name(&*error).as_str()) => format!("{daemon} isn't running."),
        zbus::Error::MethodError(_, Some(message), _) => message,
        error => error.to_string(),
    }
}
