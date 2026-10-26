//! `autark-cursors install [DIR]` draws the Autark cursor theme and writes it as an icon theme (default
//! `~/.local/share/icons/Autark`); `autark-cursors sheet FILE` draws every shape onto a PNG to look at. The shapes are
//! kept plain: the theme's blue with a thin white edge, so they stay solid on light and dark alike. Anything not drawn here falls
//! back to Adwaita, which the theme inherits.

mod draw;
mod xcursor;

use std::path::PathBuf;

use draw::CURSORS;

fn main() {
    let mut args = std::env::args().skip(1);
    let result = match args.next().as_deref() {
        Some("install") => install(args.next().map(PathBuf::from).unwrap_or_else(default_dir)),
        Some("sheet") => match args.next() {
            Some(file) => draw::sheet(&PathBuf::from(file)),
            None => Err("sheet needs a file name".to_owned()),
        },
        _ => Err("usage: autark-cursors install [DIR] | sheet FILE".to_owned()),
    };
    if let Err(error) = result {
        eprintln!("autark-cursors: {error}");
        std::process::exit(1);
    }
}

fn default_dir() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))).unwrap_or_default();
    data.join("icons").join("Autark")
}

/// Other names toolkits and the compositor ask for, and the shape each one is.
const ALIASES: &[(&str, &[&str])] = &[
    ("default", &["left_ptr", "arrow", "top_left_arrow", "left_arrow"]),
    ("text", &["xterm", "ibeam"]),
    ("pointer", &["hand1", "hand2", "pointing_hand"]),
    ("wait", &["watch"]),
    ("progress", &["left_ptr_watch", "half-busy"]),
    ("crosshair", &["cross", "tcross"]),
    ("move", &["fleur", "all-scroll", "size_all"]),
    ("ew-resize", &["sb_h_double_arrow", "h_double_arrow", "size_hor", "e-resize", "w-resize", "col-resize", "split_h", "left_side", "right_side"]),
    ("ns-resize", &["sb_v_double_arrow", "v_double_arrow", "size_ver", "n-resize", "s-resize", "row-resize", "split_v", "top_side", "bottom_side"]),
    ("nwse-resize", &["bd_double_arrow", "size_fdiag", "nw-resize", "se-resize", "top_left_corner", "bottom_right_corner"]),
    ("nesw-resize", &["fd_double_arrow", "size_bdiag", "ne-resize", "sw-resize", "top_right_corner", "bottom_left_corner"]),
    ("not-allowed", &["no-drop", "forbidden", "crossed_circle", "dnd-none"]),
    ("grab", &["openhand"]),
    ("grabbing", &["closedhand"]),
    ("help", &["question_arrow", "whats_this"]),
    ("copy", &["dnd-copy"]),
    ("alias", &["dnd-link"]),
];

fn install(dir: PathBuf) -> Result<(), String> {
    let cursors = dir.join("cursors");
    std::fs::create_dir_all(&cursors).map_err(|error| format!("{}: {error}", cursors.display()))?;
    for cursor in CURSORS {
        let file = cursors.join(cursor.name);
        std::fs::write(&file, xcursor::encode(&draw::images(cursor))).map_err(|error| format!("{}: {error}", file.display()))?;
    }
    for (target, names) in ALIASES {
        for name in *names {
            let link = cursors.join(name);
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(target, &link).map_err(|error| format!("{}: {error}", link.display()))?;
        }
    }
    let index = dir.join("index.theme");
    std::fs::write(&index, "[Icon Theme]\nName=Autark\nComment=A plain arrow and a small set of matching cursors\nInherits=Adwaita\n").map_err(|error| format!("{}: {error}", index.display()))?;
    println!("wrote {} cursors and {} names to {}", CURSORS.len(), ALIASES.iter().map(|(_, names)| names.len()).sum::<usize>(), dir.display());
    Ok(())
}
