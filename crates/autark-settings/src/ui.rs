//! The pieces every pane is made of, as the design draws them: a title, sections with a heading and an aside,
//! rounded groups of rows, and the controls that sit at a row's end.

use std::rc::Rc;

use gtk::prelude::*;

pub fn title(text: &str, note: Option<&str>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("pane-title");
    label.set_xalign(0.0);
    column.append(&label);
    if let Some(note) = note {
        let label = gtk::Label::new(Some(note));
        label.add_css_class("pane-note");
        label.set_xalign(0.0);
        label.set_wrap(true);
        column.append(&label);
    }
    column
}

/// A heading with an optional note at the right, over whatever follows.
pub fn section(heading: &str, aside: Option<&gtk::Label>, body: &impl IsA<gtk::Widget>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(heading));
    label.add_css_class("section-heading");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    top.append(&label);
    if let Some(aside) = aside {
        aside.add_css_class("note");
        top.append(aside);
    }
    column.append(&top);
    column.append(body);
    column
}

/// A rounded group; rows are separated by hairlines.
pub fn group() -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add_css_class("group");
    group
}

/// A row: its label (and a note under it), then `end` at the right.
pub fn row(label: &str, note: Option<&str>, end: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    row.add_css_class("row");
    row.append(&row_label(label, note));
    end.set_valign(gtk::Align::Center);
    row.append(end);
    row
}

pub fn row_label(label: &str, note: Option<&str>) -> gtk::Box {
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let main = gtk::Label::new(Some(label));
    main.add_css_class("row-label");
    main.set_xalign(0.0);
    text.append(&main);
    if let Some(note) = note {
        let label = gtk::Label::new(Some(note));
        label.add_css_class("note");
        label.set_xalign(0.0);
        label.set_wrap(true);
        text.append(&label);
    }
    text
}

pub fn note(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("note");
    label.set_xalign(0.0);
    label
}

/// A switch that calls `changed` when flipped by hand.
/// A value shown at a row's end, right-aligned and selectable.
pub fn value(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("value");
    label.set_xalign(1.0);
    label.set_selectable(true);
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Right);
    label
}

pub fn switch(on: bool, changed: impl Fn(bool) + 'static) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(on);
    switch.set_valign(gtk::Align::Center);
    switch.connect_state_set(move |_, on| {
        changed(on);
        gtk::glib::Propagation::Proceed
    });
    switch
}

/// A drop-down of `(value, label)` choices with `current` selected.
pub fn choice(options: &[(&str, &str)], current: &str, changed: impl Fn(&str) + 'static) -> gtk::DropDown {
    let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
    let dropdown = gtk::DropDown::from_strings(&labels);
    dropdown.add_css_class("choice");
    if let Some(index) = options.iter().position(|(value, _)| *value == current) {
        dropdown.set_selected(index as u32);
    }
    let values: Vec<String> = options.iter().map(|(value, _)| value.to_string()).collect();
    dropdown.connect_selected_notify(move |dropdown| {
        if let Some(value) = values.get(dropdown.selected() as usize) {
            changed(value);
        }
    });
    dropdown
}

/// Buttons in a pill, one pressed at a time.
pub fn segmented(options: &[(&str, &str)], current: &str, changed: impl Fn(&str) + 'static) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.add_css_class("segmented");
    let changed = Rc::new(changed);
    let mut first: Option<gtk::ToggleButton> = None;
    for (value, label) in options {
        let button = gtk::ToggleButton::with_label(label);
        button.set_active(*value == current);
        button.set_group(first.as_ref());
        let (value, changed) = (value.to_string(), changed.clone());
        button.connect_toggled(move |button| {
            if button.is_active() {
                changed(&value);
            }
        });
        first.get_or_insert(button.clone());
        bar.append(&button);
    }
    bar
}

/// A slider with words at either end; `changed` fires as it moves.
pub fn slider(min: f64, max: f64, step: f64, value: f64, left: &str, right: &str, changed: impl Fn(f64) + 'static) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    if !left.is_empty() {
        line.append(&note(left));
    }
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_value(value);
    scale.set_width_request(220);
    scale.set_draw_value(false);
    scale.connect_value_changed(move |scale| changed(scale.value()));
    line.append(&scale);
    if !right.is_empty() {
        line.append(&note(right));
    }
    line
}

pub fn button(label: &str, clicked: impl Fn(&gtk::Button) + 'static) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("small");
    button.set_valign(gtk::Align::Center);
    button.connect_clicked(clicked);
    button
}

/// Removes every child of a box.
pub fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// The pane's content in a scrolled column, as every pane is laid out.
pub fn page() -> (gtk::ScrolledWindow, gtk::Box) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.add_css_class("page");
    let scrolled = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&column).vexpand(true).build();
    (scrolled, column)
}

/// A message under a pane's title when its service isn't there.
pub fn unavailable(column: &gtk::Box, text: &str) {
    let label = note(text);
    label.add_css_class("unavailable");
    label.set_wrap(true);
    column.append(&label);
}

/// A short text field; `commit` gets its text on Enter or when focus leaves, and says whether it took it.
pub fn field(text: &str, width: i32, commit: impl Fn(&str) -> bool + 'static) -> gtk::Entry {
    let entry = gtk::Entry::builder().text(text).width_chars(width).max_width_chars(width).valign(gtk::Align::Center).build();
    entry.add_css_class("field");
    let commit = Rc::new(move |entry: &gtk::Entry| {
        if commit(&entry.text()) {
            entry.remove_css_class("invalid");
        } else {
            entry.add_css_class("invalid");
        }
    });
    entry.connect_activate({
        let commit = commit.clone();
        move |entry| commit(entry)
    });
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave({
        let entry = entry.downgrade();
        move |_| {
            if let Some(entry) = entry.upgrade() {
                commit(&entry);
            }
        }
    });
    entry.add_controller(focus);
    entry
}

/// Runs only the last of a burst of calls, `delay` after it: a slider's moves become one write.
#[derive(Clone, Default)]
pub struct Debounce(Rc<std::cell::RefCell<Option<gtk::glib::SourceId>>>);

impl Debounce {
    pub fn run(&self, delay: std::time::Duration, run: impl FnOnce() + 'static) {
        if let Some(pending) = self.0.borrow_mut().take() {
            pending.remove();
        }
        let slot = self.0.clone();
        let id = gtk::glib::timeout_add_local_once(delay, move || {
            slot.borrow_mut().take();
            run();
        });
        *self.0.borrow_mut() = Some(id);
    }
}

/// Rebuilds a page's `containers` with `build`, giving keyboard focus back to the control of the same widget
/// name (set on the controls that should keep it) or, when that control is gone, to nothing rather than to
/// whatever GTK picks, where a stray Enter would set it off.
pub fn rebuild_keeping_focus(containers: &[&gtk::Box], build: impl FnOnce()) {
    let root = containers.first().and_then(|container| container.root());
    let inside = |widget: &gtk::Widget| containers.iter().any(|container| widget.is_ancestor(*container));
    let focused = root
        .as_ref()
        .and_then(|root| root.focus())
        .filter(inside)
        .and_then(|widget| std::iter::successors(Some(widget), |widget| widget.parent()).find(|widget| widget.widget_name().contains(':')).map(|widget| widget.widget_name()));
    build();
    let Some(name) = focused else { return };
    match containers.iter().find_map(|container| named(container.upcast_ref(), &name)) {
        Some(widget) => {
            widget.grab_focus();
        }
        None => {
            if let Some(window) = root.and_downcast::<gtk::Window>() {
                GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
            }
        }
    }
}

/// Gives keyboard focus to the control in `container` with this widget name, if it's there.
pub fn focus_named(container: &gtk::Box, name: &str) {
    if let Some(widget) = named(container.upcast_ref(), name) {
        widget.grab_focus();
    }
}

/// The descendant of `root` with this widget name.
fn named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name {
        return Some(root.clone());
    }
    std::iter::successors(root.first_child(), |child| child.next_sibling()).find_map(|child| named(&child, name))
}
