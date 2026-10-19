//! The window: each group of shortcuts in turn, a name on the left and its keys as keycaps on the right.

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::shortcuts::{self, Group};

fn keycaps(combo: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_halign(gtk::Align::End);
    for cap in shortcuts::caps(combo) {
        let label = gtk::Label::new(Some(&cap));
        label.add_css_class("cap");
        row.append(&label);
    }
    row
}

fn group(group: &Group) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    section.add_css_class("group");
    let heading = gtk::Label::builder().label(group.name.to_uppercase()).xalign(0.0).build();
    heading.add_css_class("heading");
    section.append(&heading);
    for (label, combos) in &group.rows {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        row.add_css_class("row");
        let name = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).build();
        row.append(&name);
        // Several ways to do it are told apart by "or".
        let keys = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        for (n, combo) in combos.iter().enumerate() {
            if n > 0 {
                let or = gtk::Label::new(Some("or"));
                or.add_css_class("or");
                keys.append(&or);
            }
            keys.append(&keycaps(combo));
        }
        row.append(&keys);
        section.append(&row);
    }
    section
}

pub fn build(app: &gtk::Application) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder().application(app).title("Keyboard Shortcuts").default_width(720).default_height(720).build();
    window.add_css_class("keys");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let controls = gtk::WindowControls::new(gtk::PackType::Start);
    controls.set_decoration_layout(Some("close,minimize,maximize:"));
    let title = gtk::Label::builder().label("Keyboard shortcuts").xalign(0.0).hexpand(true).build();
    title.add_css_class("title");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    top.add_css_class("top");
    top.append(&controls);
    top.append(&title);
    let handle = gtk::WindowHandle::builder().child(&top).build();

    let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    list.add_css_class("list");
    let path = shortcuts::path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let groups = shortcuts::groups(&text);
            if groups.is_empty() {
                list.append(&gtk::Label::new(Some("No shortcuts are set.")));
            }
            for entry in &groups {
                list.append(&group(entry));
            }
        }
        Err(error) => {
            let message = gtk::Label::new(Some(&format!("Couldn’t read {}: {error}", path.display())));
            message.add_css_class("problem");
            list.append(&message);
        }
    }
    let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&list).build();
    let foot = gtk::Label::builder().label("Change them in Settings › Keyboard. Esc closes this.").xalign(0.0).build();
    foot.add_css_class("foot");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&handle);
    content.append(&scroller);
    content.append(&foot);
    window.set_child(Some(&content));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let window = window.downgrade();
        move |_, key, _, _| {
            if key == gdk::Key::Escape
                && let Some(window) = window.upgrade()
            {
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
    window
}
