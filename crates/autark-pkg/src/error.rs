//! One error type: a message for the person running the command, with what was being done.

use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Error(message.into()))
}

/// Adds what failed (a path, or a short phrase) to any error.
pub trait Context<T> {
    fn ctx(self, what: impl Display) -> Result<T>;
}

/// Paths and phrases both name what failed.
pub trait Display {
    fn show(&self) -> String;
}

impl Display for &Path {
    fn show(&self) -> String {
        self.display().to_string()
    }
}

impl Display for &std::path::PathBuf {
    fn show(&self) -> String {
        self.display().to_string()
    }
}

impl Display for &str {
    fn show(&self) -> String {
        (*self).to_owned()
    }
}

impl Display for String {
    fn show(&self) -> String {
        self.clone()
    }
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn ctx(self, what: impl Display) -> Result<T> {
        self.map_err(|error| Error(format!("{}: {error}", what.show())))
    }
}
