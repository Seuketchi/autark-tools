//! `terminal.toml`: every field is optional, and open windows pick up a change as it's saved.
//!
//! ```toml
//! scheme = "follow"      # "follow" the desktop's light or dark, or force "light" or "dark" for the terminal's body
//! font = "Geist Mono"
//! font_size = 13         # pixels
//! line_height = 1.7      # 1.0 to 2.0
//! scrollback = 10000     # lines kept
//! cursor = "block"       # block, bar or underline
//! cursor_blink = false
//! foreground = "#3E3A34" # any of these three replace the scheme's colour
//! background = "#EFECE5"
//! cursor_color = "#24221E"
//! palette = ["#3A3731", …] # sixteen colours, black … white then the bright ones
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    #[default]
    Follow,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cursor {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub scheme: Scheme,
    pub font: String,
    pub font_size: f64,
    pub line_height: f64,
    pub scrollback: u32,
    pub cursor: Cursor,
    pub cursor_blink: bool,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor_color: Option<String>,
    pub palette: Option<Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scheme: Scheme::Follow,
            font: "Geist Mono".to_owned(),
            font_size: 13.0,
            line_height: 1.7,
            scrollback: 10_000,
            cursor: Cursor::Block,
            cursor_blink: false,
            foreground: None,
            background: None,
            cursor_color: None,
            palette: None,
        }
    }
}

impl Config {
    /// What was asked for, with sizes held to what a terminal can show.
    fn sane(mut self) -> Self {
        self.font_size = self.font_size.clamp(8.0, 48.0);
        self.line_height = self.line_height.clamp(1.0, 2.0);
        self.scrollback = self.scrollback.min(1_000_000);
        if self.font.trim().is_empty() {
            self.font = Self::default().font;
        }
        self
    }
}

pub fn load() -> Config {
    autark_common::config::load::<Config>("terminal").sane()
}

#[derive(Default)]
struct State {
    current: Option<Rc<Config>>,
    /// Each returns whether it's still wanted, so a closed tab's drops out.
    listeners: Vec<Rc<dyn Fn(&Config) -> bool>>,
    watch: Option<autark_common::watch::Watch>,
}

thread_local!(static STATE: RefCell<State> = RefCell::default());

/// The config as it is now.
pub fn current() -> Rc<Config> {
    STATE.with_borrow_mut(|state| state.current.get_or_insert_with(|| Rc::new(load())).clone())
}

/// Calls `changed` whenever `terminal.toml` changes, until it returns false.
pub fn on_change(changed: impl Fn(&Config) -> bool + 'static) {
    STATE.with_borrow_mut(|state| {
        state.listeners.push(Rc::new(changed));
        if state.watch.is_none() {
            state.watch = Some(autark_common::watch::config("terminal", reload));
        }
    });
}

fn reload() {
    let config = Rc::new(load());
    let listeners = STATE.with_borrow_mut(|state| {
        if state.current.as_deref() == Some(&*config) {
            return Vec::new();
        }
        state.current = Some(config.clone());
        state.listeners.clone()
    });
    let wanted: Vec<bool> = listeners.iter().map(|listener| listener(&config)).collect();
    STATE.with_borrow_mut(|state| {
        let mut wanted = wanted.into_iter();
        state.listeners.retain(|_| wanted.next().unwrap_or(true));
    });
}
