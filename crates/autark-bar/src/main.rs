//! Top bar: app menu, clock, Wi-Fi, battery, volume, notification centre button.

mod app_name;
mod battery;
mod clock;
mod config;
mod logo;
mod menu;
mod volume;
mod wifi;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.autark.Bar")
        .build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build);
    app.run()
}

fn build(app: &gtk::Application) {
    // A second launch only activates the running bar.
    if app.active_window().is_some() {
        return;
    }
    let config: config::Bar = autark_common::config::load("bar");

    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_namespace(Some("autark-bar"));
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.auto_exclusive_zone_enable();

    let start = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    start.append(&logo::new());
    start.append(&app_name::new());

    let end = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    end.append(&wifi::new());
    end.append(&volume::new());
    end.append(&battery::new(config.battery_percent));
    end.append(&clock::new(&config.clock_format));

    let bar = gtk::CenterBox::new();
    bar.add_css_class("bar");
    bar.set_start_widget(Some(&start));
    bar.set_end_widget(Some(&end));
    window.set_child(Some(&bar));
    window.present();
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
