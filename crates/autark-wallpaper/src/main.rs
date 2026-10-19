//! The desktop background: a layer-shell surface behind everything on each display, showing the wallpaper
//! `theme.toml` names. That's one of the four drawn scenes, or a picture's path, which covers the display as
//! a picture set to fill would. The file is watched, so Settings' choice shows at once, and displays coming
//! and going get a background each.

use std::cell::RefCell;

use autark_common::{theme, wallpaper};
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Wallpaper").build();
    app.connect_startup(|_| theme::install(include_str!("wallpaper.css")));
    app.connect_activate(start);
    app.run()
}

thread_local!(static WINDOWS: RefCell<Vec<gtk::ApplicationWindow>> = RefCell::default());

fn start(app: &gtk::Application) {
    let Some(display) = gdk::Display::default() else { return };
    // Held for as long as the app runs, as it has no ordinary window to keep it going.
    std::mem::forget(app.hold());
    cover(app, &display);
    display.monitors().connect_items_changed({
        let app = app.clone();
        move |_, _, _, _| cover(&app, &display)
    });
    theme::on_change(|look| {
        WINDOWS.with_borrow(|windows| {
            for window in windows {
                window.set_child(Some(&content(&look.theme.wallpaper)));
            }
        })
    });
}

/// A background for each display, replacing the ones before.
fn cover(app: &gtk::Application, display: &gdk::Display) {
    let name = theme::look().theme.wallpaper.clone();
    let monitors = display.monitors();
    let windows = (0..monitors.n_items()).filter_map(|index| monitors.item(index).and_downcast::<gdk::Monitor>()).map(|monitor| background(app, &monitor, &name)).collect();
    for window in WINDOWS.replace(windows) {
        window.destroy();
    }
}

fn background(app: &gtk::Application, monitor: &gdk::Monitor, name: &str) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Background);
    window.set_namespace(Some("autark-wallpaper"));
    window.set_monitor(Some(monitor));
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(-1);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.add_css_class("wallpaper");
    window.set_child(Some(&content(name)));
    let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
    click.connect_pressed({
        let window = window.clone();
        move |_, _, x, y| menu(&window, x, y)
    });
    window.add_controller(click);
    window.present();
    window
}

/// What the desktop's right-click menu offers: a label and the command it runs.
const MENU: [(&str, &[&str]); 5] = [
    ("Change Wallpaper", &["autark-settings", "appearance"]),
    ("Display Settings", &["autark-settings", "display"]),
    ("Open Files", &["autark-files"]),
    ("Activity", &["autark-monitor"]),
    ("Settings", &["autark-settings"]),
];

/// The menu at the pointer, gone once a choice is made or a click lands elsewhere.
fn menu(window: &gtk::ApplicationWindow, x: f64, y: f64) {
    let popover = gtk::Popover::new();
    popover.set_parent(window);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for (label, command) in MENU {
        let button = gtk::Button::with_label(label);
        button.add_css_class("flat");
        button.connect_clicked({
            let popover = popover.clone();
            move |_| {
                popover.popdown();
                let command: Vec<&std::ffi::OsStr> = command.iter().map(std::ffi::OsStr::new).collect();
                if let Err(error) = gtk::gio::Subprocess::newv(&command, gtk::gio::SubprocessFlags::NONE) {
                    eprintln!("autark-wallpaper: {}: {error}", command[0].to_string_lossy());
                }
            }
        });
        list.append(&button);
    }
    popover.set_child(Some(&list));
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

/// The picture `name` gives, or the scene it names; Linen when it's neither or the picture won't load.
fn content(name: &str) -> gtk::Widget {
    if wallpaper::is_picture(name) {
        match gdk::Texture::from_filename(wallpaper::picture_path(name)) {
            Ok(texture) => {
                let picture = gtk::Picture::for_paintable(&texture);
                picture.set_content_fit(gtk::ContentFit::Cover);
                return picture.upcast();
            }
            Err(error) => eprintln!("autark-wallpaper: {name}: {error}"),
        }
    }
    let scene = *wallpaper::scene(name).unwrap_or(&wallpaper::SCENES[0]);
    let area = gtk::DrawingArea::new();
    area.set_draw_func(move |_, cr, width, height| wallpaper::draw(cr, &scene, f64::from(width), f64::from(height)));
    area.upcast()
}
