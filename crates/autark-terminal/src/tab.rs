//! One terminal: a VTE widget in the Linen colours and Geist Mono, running the user's shell.

use std::path::{Path, PathBuf};

use autark_common::theme::{self, Look};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

use crate::config::{self, Config, Cursor};
use vte::{Terminal, TerminalExt, TerminalExtManual};

/// A terminal running the user's shell in `folder` (the shell's own default when there's none). `exited` runs when the
/// shell ends, so its tab can go.
pub fn new(folder: Option<&Path>, exited: impl Fn(&Terminal) + 'static) -> Terminal {
    let terminal = Terminal::new();
    terminal.set_scroll_on_keystroke(true);
    terminal.set_scroll_on_output(false);
    terminal.set_mouse_autohide(true);
    terminal.set_audible_bell(false);
    terminal.set_allow_hyperlink(true);
    links(&terminal);
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);
    terminal.add_css_class("autark-terminal");

    restyle(&terminal, &theme::look(), &config::current());
    // The desktop's look and terminal.toml each repaint it; a closed tab's listeners let go.
    theme::on_change({
        let weak = terminal.downgrade();
        move |look| {
            if let Some(terminal) = weak.upgrade() {
                restyle(&terminal, look, &config::current());
            }
        }
    });
    config::on_change({
        let weak = terminal.downgrade();
        move |config| match weak.upgrade() {
            Some(terminal) => {
                restyle(&terminal, &theme::look(), config);
                true
            }
            None => false,
        }
    });

    terminal.connect_child_exited(move |terminal, _status| exited(terminal));
    spawn_shell(&terminal, folder);
    terminal
}

/// What counts as a link in plain text: a web address, without the full stop or bracket that ends the sentence around it.
const LINK: &str = r#"https?://[^\s<>"'`]*[^\s<>"'`.,;:!?)\]}]"#;
/// PCRE2_MULTILINE, which VTE asks of every regex it matches with.
const PCRE2_MULTILINE: u32 = 0x0000_0400;

/// Web addresses in the text, and links a program marked itself (`ls --hyperlink`, `gcc`), open with Ctrl+click: a plain
/// click is the program's when it has the mouse (tmux, vim) and starts a selection when it doesn't.
fn links(terminal: &Terminal) {
    if let Ok(regex) = vte::Regex::for_match(LINK, PCRE2_MULTILINE) {
        let tag = terminal.match_add_regex(&regex, 0);
        terminal.match_set_cursor_name(tag, "pointer");
    }
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    // Before the terminal's own handling of the click, which would otherwise take it.
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let weak = terminal.downgrade();
        move |click, _, x, y| {
            let Some(terminal) = weak.upgrade() else { return };
            if !click.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                return;
            }
            let uri = terminal.check_hyperlink_at(x, y).or_else(|| terminal.check_match_at(x, y).0);
            if let Some(uri) = uri {
                click.set_state(gtk::EventSequenceState::Claimed);
                let window = terminal.root().and_downcast::<gtk::Window>();
                gtk::UriLauncher::new(&uri).launch(window.as_ref(), None::<&gio::Cancellable>, move |result| {
                    if let Err(error) = result {
                        eprintln!("autark-terminal: couldn't open {uri}: {error}");
                    }
                });
            }
        }
    });
    terminal.add_controller(click);
}

/// Font, cursor, scrollback and colours, from the config and the desktop's look.
fn restyle(terminal: &Terminal, look: &Look, config: &Config) {
    terminal.set_font(Some(&pango::FontDescription::from_string(&format!("{} {}px", config.font, config.font_size))));
    terminal.set_cell_height_scale(config.line_height);
    terminal.set_scrollback_lines(config.scrollback.into());
    terminal.set_cursor_shape(match config.cursor {
        Cursor::Block => vte::CursorShape::Block,
        Cursor::Bar => vte::CursorShape::Ibeam,
        Cursor::Underline => vte::CursorShape::Underline,
    });
    terminal.set_cursor_blink_mode(if config.cursor_blink { vte::CursorBlinkMode::On } else { vte::CursorBlinkMode::Off });
    crate::colors::apply(terminal, look, config);
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
