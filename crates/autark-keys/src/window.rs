//! The window: each group of shortcuts in turn, a name on the left and its keys as keycaps on the right. Change waits for
//! the new keys, with Wayfire's own shortcuts held off so any of them can be taken; a combination another shortcut has is
//! named, and swapped only if you say so. Reset puts one back as the shipped `wayfire.ini` has it.

use std::cell::RefCell;
use std::rc::Rc;

use autark_common::shortcuts::{self, Group, Row};
use gtk::prelude::*;
use gtk::{gdk, glib};

/// A combination another shortcut has, waiting for the answer.
struct Clash {
    label: String,
    combo: String,
    other: String,
}

struct Editor {
    window: glib::WeakRef<gtk::ApplicationWindow>,
    list: gtk::Box,
    /// The one being changed, if any.
    capturing: RefCell<Option<String>>,
    clash: RefCell<Option<Clash>>,
    notice: gtk::Box,
    message: gtk::Label,
    swap: gtk::Button,
}

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

fn button(label: &str, clicked: impl Fn() + 'static) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("action");
    button.set_valign(gtk::Align::Center);
    button.connect_clicked(move |_| clicked());
    button
}

fn defaults() -> String {
    std::fs::read_to_string(shortcuts::DEFAULTS).unwrap_or_default()
}

impl Editor {
    fn show(self: &Rc<Self>, text: &str) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let groups = shortcuts::groups(text);
        if groups.is_empty() {
            self.list.append(&gtk::Label::new(Some("No shortcuts are set.")));
        }
        let defaults = defaults();
        for entry in &groups {
            self.list.append(&self.group(entry, &defaults));
        }
    }

    fn reload(self: &Rc<Self>) {
        match std::fs::read_to_string(shortcuts::path()) {
            Ok(text) => self.show(&text),
            Err(error) => {
                self.say(&format!("Couldn’t read {}: {error}", shortcuts::path().display()), false);
            }
        }
    }

    /// The line above the list: what's wrong, or what's being asked.
    fn say(&self, text: &str, ask: bool) {
        self.message.set_label(text);
        self.swap.set_visible(ask);
        self.notice.set_visible(!text.is_empty());
    }

    fn group(self: &Rc<Self>, group: &Group, defaults: &str) -> gtk::Box {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
        section.add_css_class("group");
        let heading = gtk::Label::builder().label(group.name.to_uppercase()).xalign(0.0).build();
        heading.add_css_class("heading");
        section.append(&heading);
        for row in &group.rows {
            section.append(&self.row(row, defaults));
        }
        section
    }

    fn row(self: &Rc<Self>, row: &Row, defaults: &str) -> gtk::Box {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        line.add_css_class("row");
        let name = gtk::Label::builder().label(&row.label).xalign(0.0).hexpand(true).build();
        line.append(&name);
        let listening = self.capturing.borrow().as_deref() == Some(row.label.as_str());
        let keys = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if listening {
            let prompt = gtk::Label::new(Some("Press the new keys. Esc cancels."));
            prompt.add_css_class("listening");
            keys.append(&prompt);
        } else {
            // Several ways to do it are told apart by "or".
            for (n, combo) in row.combos.iter().enumerate() {
                if n > 0 {
                    let or = gtk::Label::new(Some("or"));
                    or.add_css_class("or");
                    keys.append(&or);
                }
                keys.append(&keycaps(combo));
            }
        }
        line.append(&keys);
        let editable = !row.bindings.is_empty();
        if !listening && editable {
            let change = button("Change", {
                let editor = self.clone();
                let label = row.label.clone();
                move || editor.listen(&label)
            });
            change.update_property(&[gtk::accessible::Property::Label(&format!("Change the shortcut for {}", row.label.to_lowercase()))]);
            line.append(&change);
            if !defaults.is_empty() && !shortcuts::is_default(row, defaults) {
                let reset = button("Reset", {
                    let editor = self.clone();
                    let label = row.label.clone();
                    move || editor.reset(&label)
                });
                reset.update_property(&[gtk::accessible::Property::Label(&format!("Put the shortcut for {} back", row.label.to_lowercase()))]);
                line.append(&reset);
            }
        }
        line
    }

    /// Starts waiting for the new keys of the row called `label`.
    fn listen(self: &Rc<Self>, label: &str) {
        self.clash.take();
        self.say("", false);
        *self.capturing.borrow_mut() = Some(label.to_owned());
        self.hold_off(true);
        self.reload();
    }

    fn stop_listening(self: &Rc<Self>) {
        if self.capturing.take().is_some() {
            self.hold_off(false);
        }
    }

    /// Asks Wayfire to let this window have every key, or to take its own back.
    fn hold_off(&self, on: bool) {
        if let Some(toplevel) = self.window.upgrade().and_then(|window| window.surface()).and_downcast::<gdk::Toplevel>() {
            if on {
                toplevel.inhibit_system_shortcuts(None::<&gdk::Event>);
            } else {
                toplevel.restore_system_shortcuts();
            }
        }
    }

    fn apply(self: &Rc<Self>, edits: &[(String, String)]) {
        match shortcuts::write(&shortcuts::path(), edits) {
            Ok(()) => self.say("", false),
            Err(error) => self.say(&error, false),
        }
        self.reload();
    }

    fn reset(self: &Rc<Self>, label: &str) {
        let edits = shortcuts::reset(label, &defaults());
        self.apply(&edits);
    }

    /// The keys came in for the row being changed.
    fn pressed(self: &Rc<Self>, code: u32, state: gdk::ModifierType) -> glib::Propagation {
        let Some(label) = self.capturing.borrow().clone() else { return glib::Propagation::Proceed };
        let state = state & gtk::accelerator_get_default_mod_mask();
        if code == 1 && state.is_empty() {
            self.stop_listening();
            self.reload();
            return glib::Propagation::Stop;
        }
        let Some(combo) = shortcuts::combo(code, state.contains(gdk::ModifierType::CONTROL_MASK), state.contains(gdk::ModifierType::ALT_MASK), state.contains(gdk::ModifierType::SHIFT_MASK), state.contains(gdk::ModifierType::SUPER_MASK)) else {
            // A modifier on its own, or a typing key that would stop typing: keep waiting.
            return glib::Propagation::Stop;
        };
        let Ok(text) = std::fs::read_to_string(shortcuts::path()) else { return glib::Propagation::Stop };
        let groups = shortcuts::groups(&text);
        let Some(row) = groups.iter().flat_map(|group| &group.rows).find(|row| row.label == label) else { return glib::Propagation::Stop };
        if let Some(other) = shortcuts::taken_by(&groups, &combo, &label) {
            let caps = shortcuts::caps(&combo).join("+");
            self.say(&format!("{caps} is already “{}”. Swap them, or press other keys.", other.label), true);
            *self.clash.borrow_mut() = Some(Clash { label, combo, other: other.label.clone() });
            return glib::Propagation::Stop;
        }
        self.stop_listening();
        self.apply(&shortcuts::rebind(row, &combo));
        glib::Propagation::Stop
    }

    /// Gives the row being changed the keys it asked for and the row that had them the keys it had.
    fn swap(self: &Rc<Self>) {
        let Some(clash) = self.clash.take() else { return };
        let Ok(text) = std::fs::read_to_string(shortcuts::path()) else { return };
        let groups = shortcuts::groups(&text);
        let find = |label: &str| groups.iter().flat_map(|group| &group.rows).find(|row| row.label == label);
        let (Some(mine), Some(theirs)) = (find(&clash.label), find(&clash.other)) else { return };
        let mut edits = shortcuts::rebind(mine, &clash.combo);
        match mine.combos.first() {
            Some(old) => edits.extend(shortcuts::rebind(theirs, old)),
            None => edits.extend(shortcuts::rebind(theirs, "none")),
        }
        self.stop_listening();
        self.apply(&edits);
    }
}

pub fn build(app: &gtk::Application) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder().application(app).title("Keyboard Shortcuts").default_width(720).default_height(720).build();
    window.add_css_class("keys");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let title = gtk::Label::builder().label("Keyboard shortcuts").xalign(0.0).hexpand(true).build();
    title.add_css_class("title");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    top.add_css_class("top");
    top.append(&title);
    top.append(&autark_common::chrome::controls(true));
    let handle = gtk::WindowHandle::builder().child(&top).build();

    let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    list.add_css_class("list");
    let message = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).build();
    message.add_css_class("problem");
    let notice = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    notice.add_css_class("notice");
    notice.append(&message);
    notice.set_visible(false);
    let swap = gtk::Button::with_label("Swap");
    swap.add_css_class("action");
    swap.set_visible(false);
    notice.append(&swap);
    let editor = Rc::new(Editor { window: window.downgrade(), list: list.clone(), capturing: RefCell::default(), clash: RefCell::default(), notice: notice.clone(), message, swap });
    editor.swap.connect_clicked({
        let editor = editor.clone();
        move |_| editor.swap()
    });
    editor.reload();

    let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&list).build();
    let foot = gtk::Label::builder().label("Press Change, then the new keys. Esc closes this.").xalign(0.0).build();
    foot.add_css_class("foot");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&handle);
    content.append(&notice);
    content.append(&scroller);
    content.append(&foot);
    window.set_child(Some(&content));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let window = window.downgrade();
        let editor = editor.clone();
        move |_, key, code, state| {
            if editor.capturing.borrow().is_some() {
                return editor.pressed(code.saturating_sub(8), state);
            }
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
    window.connect_unmap({
        let editor = editor.clone();
        move |_| editor.stop_listening()
    });
    window
}
