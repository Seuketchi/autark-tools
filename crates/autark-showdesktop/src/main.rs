//! `autark-showdesktop`: bind it to Super+D. It minimises every open window and remembers which it hid; run it again
//! with everything hidden and it brings those back, the focused one last so it keeps focus. It's Wayfire's wm-actions
//! "show desktop" done differently on purpose: that one, as soon as any window comes back (say from the dock), brings
//! all the others back too, where here only the window you ask for returns.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use autark_common::toplevels::{self, Window};

fn main() {
    let windows: Rc<RefCell<Vec<Window>>> = Rc::default();
    let seen = windows.clone();
    let toplevels = match toplevels::watch(move |all| *seen.borrow_mut() = all.to_vec()) {
        Ok(toplevels) => toplevels,
        Err(error) => {
            eprintln!("autark-showdesktop: can't reach the compositor (is foreign-toplevel on in Wayfire?): {error}");
            std::process::exit(1);
        }
    };
    let windows = windows.borrow().clone();
    let visible: Vec<&Window> = windows.iter().filter(|window| !window.minimized).collect();
    if !visible.is_empty() {
        remember(&visible);
        for window in visible {
            toplevels.set_minimized(window.id, true);
        }
        return;
    }
    // Everything is hidden: restore what this hid, or everything when there's no record.
    let hidden = recalled();
    let mut back: Vec<(&Window, bool)> = windows.iter().filter_map(|window| hidden.iter().find(|(app, title, _)| *app == window.app_id && *title == window.title).map(|(_, _, focused)| (window, *focused))).collect();
    if back.is_empty() {
        back = windows.iter().map(|window| (window, false)).collect();
    }
    // The one that had focus goes last, so it ends up in front.
    back.sort_by_key(|(_, focused)| *focused);
    for (window, focused) in &back {
        if *focused {
            toplevels.activate(window.id);
        } else {
            toplevels.set_minimized(window.id, false);
        }
    }
    forget();
}

fn state_file() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    runtime.join("autark").join("showdesktop")
}

/// One line per window: app ID, title, whether it had focus. Window IDs change between runs, so these are how it finds them again.
fn remember(windows: &[&Window]) {
    let text: String = windows.iter().map(|window| format!("{}\t{}\t{}\n", window.app_id, window.title.replace(['\t', '\n'], " "), window.activated)).collect();
    let path = state_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

fn recalled() -> Vec<(String, String, bool)> {
    std::fs::read_to_string(state_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some((parts.next()?.to_owned(), parts.next()?.to_owned(), parts.next()? == "true"))
        })
        .collect()
}

fn forget() {
    let _ = std::fs::remove_file(state_file());
}
