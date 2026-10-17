//! Screenshots. The compositor binds Shift+Super+3 to `autark-shot screen`, Shift+Super+4 to `autark-shot area`
//! (Space there picks a window) and Shift+Super+5 to `autark-shot options`, the bar with Save to, Timer and
//! the rest. Pictures go to `~/Pictures/Screenshots` and a thumbnail shows for five seconds. Recording comes
//! later.

mod capture;
mod clipboard;
mod options;
mod overlay;
mod thumb;
mod windows;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use capture::{Image, Pixels, Shot};
use options::Options;
use overlay::Choice;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Screen,
    Area,
    Options,
}

thread_local! {
    /// The thread serving the clipboard, which the process waits for once its windows are gone.
    static CLIPBOARD: RefCell<Option<JoinHandle<()>>> = const { RefCell::new(None) };
}

const USAGE: &str = "usage: autark-shot screen|area|options [--timer SECONDS]";

fn main() -> glib::ExitCode {
    let mut command = None;
    let mut timer = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "screen" => command = Some(Command::Screen),
            "area" => command = Some(Command::Area),
            "options" => command = Some(Command::Options),
            "--timer" => match args.next().and_then(|seconds| seconds.parse::<u32>().ok()) {
                Some(seconds) => timer = Some(seconds),
                None => {
                    eprintln!("{USAGE}");
                    return glib::ExitCode::FAILURE;
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("{USAGE}");
                return glib::ExitCode::FAILURE;
            }
        }
    }
    let Some(command) = command else {
        eprintln!("{USAGE}");
        return glib::ExitCode::FAILURE;
    };
    let app = gtk::Application::builder().application_id("org.autark.Shot").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(move |app| start(app, command, timer));
    let code = app.run_with_args::<&str>(&[]);
    if let Some(serving) = CLIPBOARD.take() {
        let _ = serving.join();
    }
    code
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!("{}{}", autark_common::theme::LINEN.gtk_css(), include_str!("shot.css")));
    gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("no display"), &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn start(app: &gtk::Application, command: Command, timer: Option<u32>) {
    let options = Options::load();
    let hold = app.hold();
    let app = app.clone();
    later(timer.unwrap_or(0), move || {
        let shots = match capture::all(options.pointer) {
            Ok(shots) => shots,
            Err(error) => return eprintln!("autark-shot: {error}"),
        };
        if command == Command::Screen {
            for (index, shot) in shots.iter().enumerate() {
                deliver(&app, &shot.image, &options, index == 0);
            }
            drop(hold);
            return;
        }
        let shots = Rc::new(shots);
        let windows = windows::list();
        let app_for_done = app.clone();
        overlay::choose(&app, shots.clone(), windows, options, command == Command::Options, move |choice, mut options| {
            let _hold = hold;
            if let Choice::Area(index, x, y, w, h) = choice {
                options.area = Some((shots[index].name.clone(), x, y, w, h));
            }
            if choice != Choice::Cancel {
                options.save();
            }
            // With a timer the picture is taken afresh once it runs out, so menus opened meanwhile are in it.
            if command == Command::Options && options.timer > 0 && choice != Choice::Cancel {
                let names: Vec<String> = shots.iter().map(|shot| shot.name.clone()).collect();
                let hold = app_for_done.hold();
                let app = app_for_done.clone();
                later(options.timer, move || {
                    let _hold = hold;
                    match capture::all(options.pointer) {
                        Ok(fresh) => {
                            let name = match &choice {
                                Choice::Screen(index) | Choice::Area(index, ..) => &names[*index],
                                Choice::Cancel => return,
                            };
                            if let Some(index) = fresh.iter().position(|shot| shot.name == *name) {
                                take(&app, &fresh, &choice, index, &options);
                            }
                        }
                        Err(error) => eprintln!("autark-shot: {error}"),
                    }
                });
                return;
            }
            match &choice {
                Choice::Screen(index) | Choice::Area(index, ..) => take(&app_for_done, &shots, &choice, *index, &options),
                Choice::Cancel => {}
            }
        });
    });
}

/// Runs `then` after `seconds`, or straight away.
fn later(seconds: u32, then: impl FnOnce() + 'static) {
    if seconds == 0 {
        glib::idle_add_local_once(then);
    } else {
        glib::timeout_add_local_once(Duration::from_secs(seconds.into()), then);
    }
}

/// The chosen screen, or the chosen area of it in pixels.
fn take(app: &gtk::Application, shots: &[Shot], choice: &Choice, index: usize, options: &Options) {
    let shot = &shots[index];
    let image = match *choice {
        Choice::Area(_, x, y, w, h) => {
            let scale = shot.scale();
            let px = |value: i32| (f64::from(value.max(0)) * scale).round() as u32;
            shot.image.crop(px(x), px(y), px(w), px(h))
        }
        _ => shot.image.clone(),
    };
    deliver(app, &image, options, true);
}

pub fn texture(image: &Image) -> gdk::MemoryTexture {
    let format = match image.pixels {
        Pixels::Bgrx => gdk::MemoryFormat::B8g8r8x8,
        Pixels::Bgra => gdk::MemoryFormat::B8g8r8a8Premultiplied,
        Pixels::Rgbx => gdk::MemoryFormat::R8g8b8x8,
        Pixels::Rgba => gdk::MemoryFormat::R8g8b8a8Premultiplied,
    };
    gdk::MemoryTexture::new(image.width as i32, image.height as i32, format, &glib::Bytes::from(&image.data), image.stride as usize)
}

/// Saves, copies and shows one picture as the options say. Only the first of several goes on the clipboard.
fn deliver(app: &gtk::Application, image: &Image, options: &Options, first: bool) {
    let texture = texture(image);
    let mut saved: Option<PathBuf> = None;
    if options.save_to.folder() {
        let folder = options::folder();
        let now = glib::DateTime::now_local().expect("local time");
        let path = autark_common::files::free_name(&folder, &options::file_name(&now));
        match std::fs::create_dir_all(&folder).map_err(|error| error.to_string()).and_then(|_| texture.save_to_png(&path).map_err(|error| error.to_string())) {
            Ok(()) => saved = Some(path),
            Err(error) => eprintln!("autark-shot: saving {}: {error}", path.display()),
        }
    }
    if first && options.save_to.clipboard() {
        match clipboard::serve(texture.save_to_png_bytes().to_vec()) {
            Ok(serving) => CLIPBOARD.set(Some(serving)),
            Err(error) => eprintln!("autark-shot: copying: {error}"),
        }
    }
    if options.thumbnail && (saved.is_some() || options.save_to.clipboard()) {
        thumb::show(app, &texture, saved);
    } else if let Some(path) = saved {
        println!("{}", path.display());
    }
}
