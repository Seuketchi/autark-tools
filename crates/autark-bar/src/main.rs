//! Top bar: app menu, clock, Wi-Fi, battery, volume, Kith, notification centre button, and a Recording item while the
//! screen is being recorded.

mod app_name;
mod battery;
mod clock;
mod config;
mod kith;
mod logo;
mod menu;
mod notifications;
mod recording;
mod shared;
mod volume;
mod wifi;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use shared::Shared;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.autark.Bar")
        .build();
    app.connect_startup(|_| load_css());
    app.connect_activate(start);
    app.run()
}

/// What every monitor's bar shows, each watched once however many monitors there are.
struct Sources {
    /// Which items each bar shows; changed when `bar.toml` is, which rebuilds the bars.
    layout: RefCell<config::Layout>,
    battery_percent: Shared<bool>,
    app_name: Shared<Option<String>>,
    wifi: wifi::Source,
    volume: Shared<volume::Snapshot>,
    battery: Shared<Option<battery::Level>>,
    clock: Shared<String>,
    notifications: Shared<bool>,
    kith: Shared<kith::Kith>,
    recording: Shared<bool>,
}

fn start(app: &gtk::Application) {
    // A second launch only activates the running bar.
    thread_local!(static STARTED: Cell<bool> = const { Cell::new(false) });
    if STARTED.replace(true) {
        return;
    }
    // Keep running while no monitor is connected.
    std::mem::forget(app.hold());

    let config: config::Bar = autark_common::config::load("bar");
    let sources = Rc::new(Sources {
        layout: RefCell::new(config.layout()),
        battery_percent: Shared::new(config.battery_percent),
        app_name: app_name::source(),
        wifi: wifi::source(),
        volume: volume::source(),
        battery: battery::source(),
        clock: clock::source(&config.clock_format),
        notifications: notifications::source(),
        kith: kith::source(),
        recording: recording::source(),
    });

    let watched = sources.clone();
    let monitors = gdk::Display::default().expect("no display").monitors();
    let bars: Rc<RefCell<Vec<(gdk::Monitor, gtk::ApplicationWindow)>>> = Rc::default();
    let sync: Rc<dyn Fn()> = {
        let (app, monitors, bars, sources) = (app.clone(), monitors.clone(), bars.clone(), sources.clone());
        Rc::new(move || {
            let current: Vec<gdk::Monitor> = (0..monitors.n_items())
                .filter_map(|index| monitors.item(index).and_downcast())
                .collect();
            let mut bars = bars.borrow_mut();
            bars.retain(|(monitor, window)| {
                let keep = current.contains(monitor);
                if !keep {
                    window.destroy();
                }
                keep
            });
            for monitor in current {
                if !bars.iter().any(|(shown, _)| *shown == monitor) {
                    let window = bar(&app, &monitor, &sources);
                    bars.push((monitor, window));
                }
            }
        })
    };
    sync();
    monitors.connect_items_changed({
        let sync = sync.clone();
        move |_, _, _, _| sync()
    });

    // Settings' changes, or a hand edit, apply at once.
    let watch = autark_common::watch::config("bar", move || {
        let config: config::Bar = autark_common::config::load("bar");
        watched.battery_percent.set(config.battery_percent);
        clock::set_format(&watched.clock, &config.clock_format);
        // Other items, or the same ones in another order, need the bars built again.
        let layout = config.layout();
        if *watched.layout.borrow() != layout {
            *watched.layout.borrow_mut() = layout;
            for (_, window) in bars.take() {
                window.destroy();
            }
            sync();
        }
    });
    std::mem::forget(watch);
}

fn bar(app: &gtk::Application, monitor: &gdk::Monitor, sources: &Sources) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_monitor(Some(monitor));
    window.set_layer(Layer::Top);
    window.set_namespace(Some("autark-bar"));
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.auto_exclusive_zone_enable();
    // Panels take the keyboard only when clicked, for fields like the Wi-Fi password.
    window.set_keyboard_mode(KeyboardMode::OnDemand);

    let layout = sources.layout.borrow().clone();
    let side = |names: &[&str]| {
        let side = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for name in names {
            if let Some(item) = item(name, sources) {
                side.append(&item);
            }
        }
        side
    };
    let (start, end) = (side(&layout.start), side(&layout.end));

    let bar = gtk::CenterBox::new();
    bar.add_css_class("bar");
    bar.set_start_widget(Some(&start));
    bar.set_end_widget(Some(&end));
    window.set_child(Some(&bar));
    window.present();
    window
}

/// The bar's item called `name` (see `autark_common::layout`).
fn item(name: &str, sources: &Sources) -> Option<gtk::Widget> {
    Some(match name {
        "logo" => logo::new().upcast(),
        "app-name" => app_name::view(&sources.app_name).upcast(),
        "recording" => recording::view(&sources.recording).upcast(),
        "wifi" => wifi::view(&sources.wifi).upcast(),
        "volume" => volume::view(&sources.volume).upcast(),
        "battery" => battery::view(&sources.battery, &sources.battery_percent).upcast(),
        "kith" => kith::view(&sources.kith).upcast(),
        "clock" => clock::view(&sources.clock).upcast(),
        "notifications" => notifications::view(&sources.notifications).upcast(),
        _ => return None,
    })
}

fn load_css() {
    autark_common::theme::install(include_str!("bar.css"));
}
