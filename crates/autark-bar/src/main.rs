//! Top bar: app menu, clock, Wi-Fi, battery, volume, notification centre button.

mod app_name;
mod battery;
mod clock;
mod config;
mod logo;
mod menu;
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
    battery_percent: bool,
    app_name: Shared<Option<String>>,
    wifi: wifi::Source,
    volume: Shared<volume::Snapshot>,
    battery: Shared<Option<battery::Level>>,
    clock: Shared<String>,
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
        battery_percent: config.battery_percent,
        app_name: app_name::source(),
        wifi: wifi::source(),
        volume: volume::source(),
        battery: battery::source(),
        clock: clock::source(&config.clock_format),
    });

    let monitors = gdk::Display::default().expect("no display").monitors();
    let bars: Rc<RefCell<Vec<(gdk::Monitor, gtk::ApplicationWindow)>>> = Rc::default();
    let sync = {
        let (app, monitors) = (app.clone(), monitors.clone());
        move || {
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
        }
    };
    sync();
    monitors.connect_items_changed(move |_, _, _, _| sync());
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

    let start = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    start.append(&logo::new());
    start.append(&app_name::view(&sources.app_name));

    let end = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    end.append(&wifi::view(&sources.wifi));
    end.append(&volume::view(&sources.volume));
    end.append(&battery::view(&sources.battery, sources.battery_percent));
    end.append(&clock::view(&sources.clock));

    let bar = gtk::CenterBox::new();
    bar.add_css_class("bar");
    bar.set_start_widget(Some(&start));
    bar.set_end_widget(Some(&end));
    window.set_child(Some(&bar));
    window.present();
    window
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        "{}{}",
        autark_common::theme::LINEN.gtk_css(),
        include_str!("bar.css")
    ));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
