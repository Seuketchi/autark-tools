//! The thumbnail at the bottom right after a capture: click opens the picture, dragging drops the file into
//! an app. It goes after five seconds, or once the pointer leaves it.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const SHOWN: Duration = Duration::from_secs(5);

pub fn show(app: &gtk::Application, texture: &gdk::MemoryTexture, path: Option<PathBuf>) {
    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("shot-thumb");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("autark-shot-thumbnail"));
    window.set_keyboard_mode(KeyboardMode::None);
    for (edge, margin) in [(Edge::Bottom, 24), (Edge::Right, 24)] {
        window.set_anchor(edge, true);
        window.set_margin(edge, margin);
    }

    let picture = gtk::Picture::for_paintable(texture);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    // A picture asks for its full size; a scrolled window without scrollbars holds it to the thumbnail's.
    let fit =
        gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::External).vscrollbar_policy(gtk::PolicyType::External).min_content_width(208).min_content_height(128).child(&picture).build();
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("thumb-frame");
    frame.set_halign(gtk::Align::End);
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.append(&fit);
    let text = match &path {
        Some(_) => "Saved to Screenshots · drag it into an app, or click to open",
        None => "Copied to the clipboard",
    };
    let label = gtk::Label::new(Some(text));
    label.add_css_class("thumb-label");
    label.set_halign(gtk::Align::End);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.append(&frame);
    column.append(&label);
    window.set_child(Some(&column));

    if let Some(path) = path {
        let file = gio::File::for_path(&path);
        frame.set_cursor_from_name(Some("pointer"));
        let click = gtk::GestureClick::new();
        let (uri, weak) = (file.uri(), window.downgrade());
        click.connect_released(move |_, _, _, _| {
            if let Err(error) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
                eprintln!("autark-shot: opening {uri}: {error}");
            }
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
        frame.add_controller(click);
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY);
        drag.set_content(Some(&gdk::ContentProvider::for_value(&gdk::FileList::from_array(&[file]).to_value())));
        let paintable = texture.clone();
        drag.connect_drag_begin(move |source, _| source.set_icon(Some(&paintable), 24, 24));
        let weak = window.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
        frame.add_controller(drag);
    }

    // Closing waits while the pointer is over it, then gives a moment more.
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let arm = {
        let (timer, weak) = (timer.clone(), window.downgrade());
        move |after: Duration| {
            if let Some(old) = timer.borrow_mut().take() {
                old.remove();
            }
            let (weak, slot) = (weak.clone(), timer.clone());
            *timer.borrow_mut() = Some(glib::timeout_add_local_once(after, move || {
                slot.borrow_mut().take();
                if let Some(window) = weak.upgrade() {
                    window.close();
                }
            }));
        }
    };
    let hover = gtk::EventControllerMotion::new();
    let slot = timer.clone();
    hover.connect_enter(move |_, _, _| {
        if let Some(old) = slot.borrow_mut().take() {
            old.remove();
        }
    });
    let rearm = arm.clone();
    hover.connect_leave(move |_| rearm(Duration::from_millis(1500)));
    column.add_controller(hover);
    arm(SHOWN);
    window.present();
}
