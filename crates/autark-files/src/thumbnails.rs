//! Image thumbnails in the shared freedesktop cache (`~/.cache/thumbnails/large`), so other apps
//! reuse ours and we reuse theirs. Made off the main thread, kept in memory once loaded.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gtk::cairo::{Format, ImageSurface};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::entry::Entry;

/// Larger files aren't worth decoding for a thumbnail.
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const SIZE: i32 = 256;

type Waiting = Vec<Box<dyn FnOnce(ImageSurface)>>;
/// Each thumbnail with the file time it was made for.
type Loaded = HashMap<PathBuf, (Option<SystemTime>, ImageSurface)>;

#[derive(Clone, Default)]
pub struct Thumbnails {
    loaded: Rc<RefCell<Loaded>>,
    pending: Rc<RefCell<HashMap<PathBuf, Waiting>>>,
}

impl Thumbnails {
    /// Calls `ready` with the thumbnail, now if it's loaded, later if it has to be made, never
    /// if it can't be.
    pub fn request(&self, entry: &Entry, ready: impl FnOnce(ImageSurface) + 'static) {
        if entry.size > MAX_BYTES {
            return;
        }
        if let Some((modified, texture)) = self.loaded.borrow().get(&entry.path)
            && *modified == entry.modified
        {
            ready(texture.clone());
            return;
        }
        let mut pending = self.pending.borrow_mut();
        if let Some(waiting) = pending.get_mut(&entry.path) {
            waiting.push(Box::new(ready));
            return;
        }
        pending.insert(entry.path.clone(), vec![Box::new(ready)]);
        drop(pending);

        let (path, modified, this) = (entry.path.clone(), entry.modified, self.clone());
        glib::spawn_future_local(async move {
            let source = path.clone();
            let made = gio::spawn_blocking(move || make(&source, modified)).await.ok().flatten();
            let waiting = this.pending.borrow_mut().remove(&path).unwrap_or_default();
            let Some(texture) = made.and_then(|file| gdk::Texture::from_filename(file).ok()).as_ref().and_then(surface) else { return };
            this.loaded.borrow_mut().insert(path, (modified, texture.clone()));
            for ready in waiting {
                ready(texture.clone());
            }
        });
    }
}

/// The texture as a cairo surface, which draws at whatever size the icon is.
fn surface(texture: &gdk::Texture) -> Option<ImageSurface> {
    let mut surface = ImageSurface::create(Format::ARgb32, texture.width(), texture.height()).ok()?;
    let stride = surface.stride() as usize;
    // GDK's default memory format is cairo's ARGB32 on little-endian machines.
    texture.download(&mut surface.data().ok()?, stride);
    surface.mark_dirty();
    Some(surface)
}

fn seconds(time: Option<SystemTime>) -> u64 {
    time.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |since| since.as_secs())
}

/// The cached thumbnail's path, making it if it's missing or older than the file.
fn make(path: &Path, modified: Option<SystemTime>) -> Option<PathBuf> {
    let uri = gio::File::for_path(path).uri();
    let name = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri.as_str())?;
    let dir = glib::user_cache_dir().join("thumbnails/large");
    let thumbnail = dir.join(format!("{name}.png"));
    let mtime = seconds(modified).to_string();
    if let Ok(cached) = Pixbuf::from_file(&thumbnail)
        && cached.option("tEXt::Thumb::MTime").is_some_and(|value| value == mtime)
    {
        return Some(thumbnail);
    }
    // Thumbnails of files in the cache itself would only feed on each other.
    if path.starts_with(glib::user_cache_dir().join("thumbnails")) {
        return None;
    }
    let pixbuf = Pixbuf::from_file_at_scale(path, SIZE, SIZE, true).ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let _ = std::fs::set_permissions(glib::user_cache_dir().join("thumbnails"), std::os::unix::fs::PermissionsExt::from_mode(0o700));
    let temporary = dir.join(format!(".{name}.{}.png", std::process::id()));
    pixbuf.savev(&temporary, "png", &[("tEXt::Thumb::URI", uri.as_str()), ("tEXt::Thumb::MTime", mtime.as_str())]).ok()?;
    std::fs::rename(&temporary, &thumbnail).ok()?;
    Some(thumbnail)
}
