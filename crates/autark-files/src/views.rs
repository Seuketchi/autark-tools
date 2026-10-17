//! The icon and list views over one shared, sorted selection model. Each item takes a right
//! click for its menu, can be dragged out, and a folder takes drops.

use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::entry::{self, Entry, Sort};
use crate::icons;
use crate::thumbnails::Thumbnails;

pub type OnMenu = Rc<dyn Fn(Option<u32>, &gtk::Widget, f64, f64)>;
/// Paths dropped, the folder they were dropped on, and whether to copy rather than move.
pub type OnDrop = Rc<dyn Fn(Vec<PathBuf>, PathBuf, bool) -> bool>;
pub type DragFiles = Rc<dyn Fn(u32) -> Vec<gio::File>>;

pub struct Hooks {
    pub on_menu: OnMenu,
    pub on_drop: OnDrop,
    pub drag_files: DragFiles,
    pub thumbnails: Thumbnails,
}

pub fn entry_of(object: &glib::Object) -> Option<Entry> {
    object.downcast_ref::<glib::BoxedAnyObject>().map(|boxed| boxed.borrow::<Entry>().clone())
}

pub fn object(entry: Entry) -> glib::BoxedAnyObject {
    glib::BoxedAnyObject::new(entry)
}

/// Whether the drop should copy: only when the source can't let go of the files (another app's
/// read-only view, say). Moves across disks copy and then remove, as Finder's would not, but
/// the files end up in one place either way.
fn copies(target: &gtk::DropTarget) -> bool {
    target.current_drop().is_some_and(|drop| !drop.actions().contains(gdk::DragAction::MOVE))
}

/// Gives a dropped file list as paths.
fn paths(value: &glib::Value) -> Vec<PathBuf> {
    value.get::<gdk::FileList>().map(|list| list.files().iter().filter_map(|file| file.path()).collect()).unwrap_or_default()
}

/// A drop target taking files into `folder` (asked for at drop time).
pub fn drop_target(hooks: &Rc<Hooks>, folder: impl Fn() -> Option<PathBuf> + 'static) -> gtk::DropTarget {
    let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
    let folder = Rc::new(folder);
    let accepts = folder.clone();
    target.connect_accept(move |_, _| accepts().is_some());
    let hooks = hooks.clone();
    target.connect_drop(move |target, value, _, _| {
        let Some(folder) = folder() else { return false };
        (hooks.on_drop)(paths(value), folder, copies(target))
    });
    target
}

/// Right click, drag and drop for the widget showing one item.
fn attach(widget: &gtk::Widget, list_item: &gtk::ListItem, hooks: &Rc<Hooks>) {
    let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
    let (item, menu_hooks) = (list_item.downgrade(), hooks.clone());
    click.connect_pressed(move |gesture, _, x, y| {
        let Some(item) = item.upgrade() else { return };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(widget) = gesture.widget() {
            (menu_hooks.on_menu)(Some(item.position()), &widget, x, y);
        }
    });
    widget.add_controller(click);

    let drag = gtk::DragSource::builder().actions(gdk::DragAction::COPY | gdk::DragAction::MOVE).build();
    let (item, drag_hooks) = (list_item.downgrade(), hooks.clone());
    drag.connect_prepare(move |_, _, _| {
        let item = item.upgrade()?;
        let files = (drag_hooks.drag_files)(item.position());
        (!files.is_empty()).then(|| gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value()))
    });
    widget.add_controller(drag);

    let item = list_item.downgrade();
    widget.add_controller(drop_target(hooks, move || {
        let entry = item.upgrade()?.item().as_ref().and_then(entry_of)?;
        (entry.is_dir && entry.trashed.is_none()).then_some(entry.path)
    }));
}

pub fn grid(model: &gtk::MultiSelection, hooks: &Rc<Hooks>) -> gtk::GridView {
    let factory = gtk::SignalListItemFactory::new();
    let setup_hooks = hooks.clone();
    factory.connect_setup(move |_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
        let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        holder.set_halign(gtk::Align::Center);
        holder.set_size_request(72, 58);
        // Two lines at most, as Finder wraps long names.
        let name = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).lines(2).justify(gtk::Justification::Center).max_width_chars(16).build();
        name.add_css_class("name");
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 8);
        tile.add_css_class("tile");
        tile.set_size_request(116, -1);
        tile.append(&holder);
        tile.append(&name);
        attach(tile.upcast_ref(), list_item, &setup_hooks);
        list_item.set_child(Some(&tile));
    });
    let bind_hooks = hooks.clone();
    factory.connect_bind(move |_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
        let (Some(entry), Some(tile)) = (list_item.item().as_ref().and_then(entry_of), list_item.child()) else { return };
        let Some(holder) = tile.first_child().and_downcast::<gtk::Box>() else { return };
        while let Some(child) = holder.first_child() {
            holder.remove(&child);
        }
        holder.append(&icons::icon(&entry, 72, 58, &bind_hooks.thumbnails));
        if let Some(name) = holder.next_sibling().and_downcast::<gtk::Label>() {
            name.set_label(&entry.name);
        }
        tile.set_tooltip_text(Some(&entry.name));
    });
    let grid = gtk::GridView::builder().model(model).factory(&factory).min_columns(2).max_columns(12).enable_rubberband(true).build();
    grid.add_css_class("icons");
    // Columns from the tiles' minimum width, so as many fit as the design shows.
    grid.set_hscroll_policy(gtk::ScrollablePolicy::Minimum);
    grid
}

/// One list column: a label per item, filled by `text`.
fn column(title: &str, hooks: &Rc<Hooks>, text: fn(&Entry) -> String, by: Sort) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_hooks = hooks.clone();
    factory.connect_setup(move |_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        label.add_css_class("sub");
        attach(label.upcast_ref(), list_item, &setup_hooks);
        list_item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
        if let (Some(entry), Some(label)) = (list_item.item().as_ref().and_then(entry_of), list_item.child().and_downcast::<gtk::Label>()) {
            label.set_label(&text(&entry));
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_fixed_width(130);
    column.set_sorter(Some(&sorter(by)));
    column
}

pub fn sorter(by: Sort) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| {
        let (Some(a), Some(b)) = (a.downcast_ref::<glib::BoxedAnyObject>(), b.downcast_ref::<glib::BoxedAnyObject>()) else { return gtk::Ordering::Equal };
        let (a, b) = (a.borrow::<Entry>(), b.borrow::<Entry>());
        // Folders first, as in the icon view.
        b.is_dir.cmp(&a.is_dir).then_with(|| entry::compare(&a, &b, by)).into()
    })
}

pub fn list(model: &gtk::MultiSelection, hooks: &Rc<Hooks>) -> gtk::ColumnView {
    let factory = gtk::SignalListItemFactory::new();
    let setup_hooks = hooks.clone();
    factory.connect_setup(move |_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        cell.set_hexpand(true);
        let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        cell.append(&gtk::Box::new(gtk::Orientation::Horizontal, 0));
        cell.append(&label);
        attach(cell.upcast_ref(), list_item, &setup_hooks);
        list_item.set_child(Some(&cell));
    });
    factory.connect_bind(move |_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
        let (Some(entry), Some(cell)) = (list_item.item().as_ref().and_then(entry_of), list_item.child()) else { return };
        let Some(holder) = cell.first_child().and_downcast::<gtk::Box>() else { return };
        while let Some(child) = holder.first_child() {
            holder.remove(&child);
        }
        holder.append(&icons::mini(&entry));
        if let Some(label) = holder.next_sibling().and_downcast::<gtk::Label>() {
            label.set_label(&entry.name);
        }
    });
    let name = gtk::ColumnViewColumn::new(Some("NAME"), Some(factory));
    name.set_expand(true);
    name.set_sorter(Some(&sorter(Sort::Name)));

    let view = gtk::ColumnView::builder().model(model).show_column_separators(false).show_row_separators(false).enable_rubberband(true).build();
    view.add_css_class("rows");
    view.append_column(&name);
    view.append_column(&column("SIZE", hooks, |entry| if entry.is_dir { "–".to_owned() } else { entry::size(entry.size) }, Sort::Size));
    view.append_column(&column("MODIFIED", hooks, |entry| entry::date(entry.modified), Sort::Modified));
    view.append_column(&column("KIND", hooks, Entry::kind, Sort::Kind));
    view.sort_by_column(Some(&name), gtk::SortType::Ascending);
    view
}
