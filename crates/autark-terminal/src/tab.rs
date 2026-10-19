//! One terminal: a VTE widget in the Linen colours and Geist Mono, running the user's shell.

use std::path::{Path, PathBuf};

use autark_common::theme;
use gtk::prelude::*;
use gtk::{gio, glib, pango};
use vte::{Terminal, TerminalExt, TerminalExtManual};

const FONT: &str = "Geist Mono 13px";

/// A terminal running the user's shell in `folder` (the shell's own default when there's none). `exited` runs when the
/// shell ends, so its tab can go.
pub fn new(folder: Option<&Path>, exited: impl Fn(&Terminal) + 'static) -> Terminal {
    let terminal = Terminal::new();
    terminal.set_font(Some(&pango::FontDescription::from_string(FONT)));
    terminal.set_cell_height_scale(1.7);
    terminal.set_cursor_shape(vte::CursorShape::Block);
    terminal.set_scrollback_lines(10_000);
    terminal.set_scroll_on_keystroke(true);
    terminal.set_scroll_on_output(false);
    terminal.set_mouse_autohide(true);
    terminal.set_audible_bell(false);
    terminal.set_allow_hyperlink(true);
    terminal.set_cursor_blink_mode(vte::CursorBlinkMode::System);
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);
    terminal.add_css_class("autark-terminal");

    let look = theme::look();
    crate::colors::apply(&terminal, &look.palette, look.dark);
    let weak = terminal.downgrade();
    theme::on_change(move |look| {
        if let Some(terminal) = weak.upgrade() {
            crate::colors::apply(&terminal, &look.palette, look.dark);
        }
    });

    terminal.connect_child_exited(move |terminal, _status| exited(terminal));
    spawn_shell(&terminal, folder);
    terminal
}

fn spawn_shell(terminal: &Terminal, folder: Option<&Path>) {
    let shell = std::env::var("SHELL").ok().filter(|shell| !shell.is_empty()).unwrap_or_else(|| "/bin/bash".to_owned());
    // An empty environment list would mean an empty environment, so pass ours, with what a terminal promises programs.
    let mut env: Vec<String> =
        std::env::vars().filter(|(key, _)| !matches!(key.as_str(), "TERM" | "COLORTERM")).map(|(key, value)| format!("{key}={value}")).collect();
    env.push("TERM=xterm-256color".to_owned());
    env.push("COLORTERM=truecolor".to_owned());
    // The design's title is "user@host: folder". Bash only says that with a prompt hook, so add one to any the user has.
    let title = r#"printf '\033]0;%s@%s: %s\007' "$USER" "${HOSTNAME%%.*}" "${PWD/#$HOME/$__autark_home}""#;
    let title = format!("__autark_home='~';{title}");
    let hook = match std::env::var("PROMPT_COMMAND") {
        Ok(existing) if !existing.is_empty() => format!("{title};{existing}"),
        _ => title,
    };
    env.retain(|entry| !entry.starts_with("PROMPT_COMMAND="));
    env.push(format!("PROMPT_COMMAND={hook}"));
    let env: Vec<&str> = env.iter().map(String::as_str).collect();
    let folder = folder.and_then(Path::to_str);
    terminal.spawn_async(
        vte::PtyFlags::DEFAULT,
        folder,
        &[shell.as_str()],
        &env,
        glib::SpawnFlags::SEARCH_PATH,
        || {},
        -1,
        None::<&gio::Cancellable>,
        |result| {
            if let Err(error) = result {
                eprintln!("autark-terminal: can't start the shell: {error}");
            }
        },
    );
}

/// The folder the shell in `terminal` is in, so a new tab can open there.
pub fn folder(terminal: &Terminal) -> Option<PathBuf> {
    let uri = terminal.current_directory_uri()?;
    gio::File::for_uri(&uri).path()
}
