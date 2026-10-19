//! Default Apps: which app opens each kind of thing, kept in `~/.config/mimeapps.list` through GIO as every desktop
//! does, so the file manager, the browser and `gio open` all agree. The terminal isn't a MIME type; it's the command
//! Super+Enter runs, in `wayfire.ini`.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use crate::{ui, wayfire};

/// A kind of thing, a note on it, and the MIME types that move together.
const KINDS: &[(&str, &str, &[&str])] = &[
    ("Web browser", "Links and web pages", &["x-scheme-handler/https", "x-scheme-handler/http", "text/html"]),
    ("Email", "mailto: links", &["x-scheme-handler/mailto"]),
    ("Files", "Folders", &["inode/directory"]),
    ("Text editor", "Plain text", &["text/plain"]),
    ("Images", "Pictures", &["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml"]),
    ("Video", "Movies and clips", &["video/mp4", "video/x-matroska", "video/webm"]),
    ("Music", "Songs and podcasts", &["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav"]),
    ("PDF", "Documents", &["application/pdf"]),
    ("Archives", "Zip and tar files", &["application/zip", "application/x-tar", "application/gzip"]),
];

const TERMINALS: [(&str, &str); 3] = [("autark-terminal", "Terminal"), ("foot", "Foot"), ("footclient", "Foot (server)")];

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Default Apps", Some("What opens when you open something.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);

    let group = ui::group();
    for &(label, note, mimes) in KINDS {
        group.append(&ui::row(label, Some(note), &chooser(mimes, &status)));
    }
    column.append(&ui::section("Apps", None, &group));

    let terminal = ui::group();
    let installed: Vec<(&str, &str)> = TERMINALS.iter().copied().filter(|(command, _)| on_path(command)).collect();
    if installed.is_empty() {
        terminal.append(&ui::row("Terminal", None, &ui::note("None found")));
    } else {
        let current = wayfire::get("command/command_terminal").unwrap_or_default();
        let current = installed.iter().find(|(command, _)| *command == current.trim()).map_or("", |(command, _)| *command);
        let status = status.clone();
        let dropdown = ui::choice(&installed, current, move |command| {
            let result = wayfire::set(&[("command/command_terminal", command.to_owned()), ("command/command_terminal_alt", command.to_owned())]);
            status.set_visible(result.is_err());
            if let Err(error) = result {
                status.set_label(&error);
            }
        });
        terminal.append(&ui::row("Terminal", Some("Opened by Super+Enter and Ctrl+Alt+T"), &dropdown));
    }
    column.append(&ui::section("Shortcuts", None, &terminal));
    column.append(&status);
    page.upcast()
}

/// A drop-down of the apps that handle `mimes[0]`, the current default selected; choosing one makes it the default for
/// every type in `mimes`.
fn chooser(mimes: &'static [&'static str], status: &gtk::Label) -> gtk::Widget {
    let apps: Vec<gio::AppInfo> = gio::AppInfo::all_for_type(mimes[0]).into_iter().filter(|app| app.should_show()).collect();
    if apps.is_empty() {
        return ui::note("Nothing installed handles this").upcast();
    }
    let ids: Vec<String> = apps.iter().map(|app| app.id().map(|id| id.to_string()).unwrap_or_default()).collect();
    let names: Vec<String> = apps.iter().map(|app| app.name().to_string()).collect();
    let current = gio::AppInfo::default_for_type(mimes[0], false).and_then(|app| app.id()).map(|id| id.to_string()).unwrap_or_default();
    let options: Vec<(&str, &str)> = ids.iter().zip(&names).map(|(id, name)| (id.as_str(), name.as_str())).collect();
    let apps = Rc::new(apps);
    let status = status.clone();
    ui::choice(&options, &current, {
        let ids = ids.clone();
        move |id| {
            let Some(app) = ids.iter().position(|known| known == id).and_then(|index| apps.get(index)) else { return };
            let result = mimes.iter().try_for_each(|mime| app.set_as_default_for_type(mime));
            status.set_visible(result.is_err());
            if let Err(error) = result {
                status.set_label(&format!("Couldn't set the default: {error}"));
            }
        }
    })
    .upcast()
}

fn on_path(command: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(command).is_file()))
}
