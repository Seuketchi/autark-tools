//! Archives. `autark-archive file…` opens each in a window to look inside and take things out. Without a window:
//! `autark-archive extract file…` unpacks each beside itself, and `autark-archive compress [--tar] file…` packs files
//! from one folder into a zip (or `tar.gz`) beside them. Nothing is ever overwritten.

mod archive;
mod window;

use std::path::PathBuf;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("extract") => return finish(args[1..].iter().map(|file| extract(PathBuf::from(file))).collect()),
        Some("compress") => {
            let tar = args.iter().any(|arg| arg == "--tar");
            let files: Vec<PathBuf> = args[1..].iter().filter(|arg| *arg != "--tar").map(PathBuf::from).collect();
            return finish(vec![archive::compress(&files, tar).map(|path| println!("{}", path.display()))]);
        }
        _ => {}
    }
    // Each run has its window; `HANDLES_OPEN` gives it the files from the command line.
    let app = gtk::Application::builder().application_id("org.autark.Archive").flags(gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("archive.css")));
    app.connect_activate(|app| window::Window::new(app).window.present());
    app.connect_open(|app, files, _| {
        for file in files {
            let window = window::Window::new(app);
            match file.path() {
                Some(path) => window.open(&path),
                None => window.fail("Only files on this computer can be opened."),
            }
            window.window.present();
        }
    });
    app.run()
}

/// Unpacks `file` beside itself and says where it went.
fn extract(file: PathBuf) -> Result<(), String> {
    let entries = archive::list(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    let dest = file.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new(".")).to_owned();
    for path in archive::extract(&file, &dest, &[], &entries).map_err(|error| format!("{}: {error}", file.display()))? {
        println!("{}", path.display());
    }
    Ok(())
}

fn finish(results: Vec<Result<(), String>>) -> glib::ExitCode {
    let mut code = glib::ExitCode::SUCCESS;
    for result in results {
        if let Err(error) = result {
            eprintln!("autark-archive: {error}");
            code = glib::ExitCode::FAILURE;
        }
    }
    code
}
