//! Small pieces every pane is made of: a form field, a line of feedback, a section with a heading, a page that scrolls.

use gtk::prelude::*;

pub(crate) fn entry(placeholder: &str, width: i32) -> gtk::Entry {
    gtk::Entry::builder().placeholder_text(placeholder).width_chars(width).max_width_chars(width).build()
}

/// A line of feedback under a form. It takes no room until it has something to say.
pub(crate) fn note() -> gtk::Label {
    let label = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    label.add_css_class("note");
    label.connect_label_notify(|label| label.set_visible(!label.label().is_empty()));
    label
}

pub(crate) fn dim(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    label.add_css_class("dim");
    label
}

pub(crate) fn rows() -> gtk::Box {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("rows");
    list
}

pub(crate) fn section(title: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class("section");
    let heading = gtk::Label::builder().label(title).xalign(0.0).build();
    heading.add_css_class("heading");
    section.append(&heading);
    section
}

pub(crate) fn line(children: &[&gtk::Widget]) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for child in children {
        line.append(*child);
    }
    line
}

pub(crate) fn clear(list: &gtk::Box) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

/// One tab's page: a scrolled column with the `page` class.
pub(crate) fn page() -> (gtk::ScrolledWindow, gtk::Box) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
    page.add_css_class("page");
    let scrolled = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).hexpand(true).vexpand(true).child(&page).build();
    (scrolled, page)
}

pub(crate) fn row(text: &str, secondary: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("row");
    let main = gtk::Label::builder().label(text).xalign(0.0).hexpand(true).build();
    let side = gtk::Label::builder().label(secondary).xalign(1.0).build();
    side.add_css_class("dim");
    row.append(&main);
    row.append(&side);
    row
}

pub(crate) fn small_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("flat");
    button.add_css_class("small");
    button
}

/// An amount for a box you type in: `2487` or `2487.50`, never more places than it needs.
pub(crate) fn pesos_text(amount: autark_finance::money::Money) -> String {
    let text = format!("{:.2}", amount.pesos_f64());
    text.strip_suffix(".00").map_or(text.clone(), str::to_string)
}
