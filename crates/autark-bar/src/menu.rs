//! Bar items that open a panel when clicked, styled like the notification centre's cards.

use gtk::prelude::*;

/// A flat bar button showing `child` that opens `content` in a panel below it.
pub fn button(child: &impl IsA<gtk::Widget>, content: &impl IsA<gtk::Widget>, classes: &[&str]) -> gtk::MenuButton {
    let popover = gtk::Popover::builder().has_arrow(false).child(content).build();
    popover.add_css_class("panel");
    let button = gtk::MenuButton::builder().child(child).popover(&popover).build();
    button.add_css_class("item");
    for class in classes {
        button.add_css_class(class);
    }
    button
}

/// A panel heading such as "Sound".
pub fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("heading");
    label
}

/// A smaller grey label above a group, such as "Output".
pub fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("caption");
    label
}

/// A full-width row in a panel list, with a check mark when it's the current choice.
pub fn row(text: &str, current: bool) -> gtk::Button {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(30)
        .build();
    let check = gtk::Label::new(Some("✓"));
    check.add_css_class("check");
    check.set_opacity(if current { 1.0 } else { 0.0 });
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.append(&label);
    content.append(&check);
    let row = gtk::Button::builder().child(&content).build();
    row.add_css_class("row");
    row
}
