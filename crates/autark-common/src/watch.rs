//! Following files from a GTK tool: Settings (or you, by hand) changes a config file and the tool picks it up
//! at once. Writes come as bursts of events (an atomic write is a create and a rename), so the callback runs
//! once, a moment after the last.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gio::prelude::*;

/// Keeps the monitors alive; dropping it stops the watching.
pub struct Watch {
    _monitors: Vec<gio::FileMonitor>,
}

/// Calls `changed` after `<name>.toml` changes, in the user's config or the system's.
pub fn config(name: &str, changed: impl Fn() + 'static) -> Watch {
    files(&crate::config::paths(name), changed)
}

/// Calls `changed` after any of `paths` is written, created or removed. They needn't exist yet.
pub fn files(paths: &[impl AsRef<Path>], changed: impl Fn() + 'static) -> Watch {
    let changed = Rc::new(changed);
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let monitors = paths
        .iter()
        .filter_map(|path| {
            let monitor = gio::File::for_path(path.as_ref()).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE).ok()?;
            let (changed, pending) = (changed.clone(), pending.clone());
            monitor.connect_changed(move |_, _, _, event| {
                if matches!(event, gio::FileMonitorEvent::AttributeChanged | gio::FileMonitorEvent::PreUnmount | gio::FileMonitorEvent::Unmounted) {
                    return;
                }
                if let Some(earlier) = pending.borrow_mut().take() {
                    earlier.remove();
                }
                let (changed, slot) = (changed.clone(), pending.clone());
                let id = glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    slot.borrow_mut().take();
                    changed();
                });
                *pending.borrow_mut() = Some(id);
            });
            Some(monitor)
        })
        .collect();
    Watch { _monitors: monitors }
}
