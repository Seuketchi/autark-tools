//! The info pane: the selected item's icon or preview, what it is, when it changed, where it
//! is, and Open (or Put Back in the Trash). Text files show their first lines.

use std::io::Read;

use gtk::prelude::*;

use crate::entry::{self, Entry};
use crate::icons;
use crate::thumbnails::Thumbnails;

/// How much of a text file the preview reads.
const PREVIEW_BYTES: usize = 2048;
const PREVIEW_LINES: usize = 14;

pub struct Info {
    pub root: gtk::Box,
    art: gtk::Box,
    pub name: gtk::Label,
    kind: gtk::Label,
    details: gtk::Grid,
    preview: gtk::Label,
    pub open: gtk::Button,
    empty: gtk::Label,
    thumbnails: Thumbnails,
}

impl Info {
    pub fn new(thumbnails: Thumbnails) -> Self {
        let art = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        art.set_halign(gtk::Align::Center);
        let name = gtk::Label::builder().justify(gtk::Justification::Center).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(22).build();
        name.add_css_class("info-name");
        let kind = gtk::Label::builder().justify(gtk::Justification::Center).wrap(true).max_width_chars(28).build();
        kind.add_css_class("info-kind");
        let details = gtk::Grid::builder().row_spacing(8).column_spacing(12).build();
        details.add_css_class("details");
        let preview = gtk::Label::builder().xalign(0.0).yalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(30).build();
        preview.add_css_class("preview");
        let open = gtk::Button::with_label("Open");
        open.add_css_class("open");
        let empty = gtk::Label::new(Some("Select an item to preview it"));
        empty.add_css_class("empty");
        empty.set_margin_top(120);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
        root.add_css_class("info");
        root.set_size_request(248, -1);
        root.set_hexpand(false);
        for widget in [art.upcast_ref::<gtk::Widget>(), name.upcast_ref(), kind.upcast_ref(), details.upcast_ref(), open.upcast_ref(), preview.upcast_ref(), empty.upcast_ref()] {
            root.append(widget);
        }
        Self { root, art, name, kind, details, preview, open, empty, thumbnails }
    }

    pub fn show(&self, selected: &[Entry]) {
        let one = match selected {
            [one] => Some(one),
            _ => None,
        };
        for widget in [self.art.upcast_ref::<gtk::Widget>(), self.name.upcast_ref(), self.kind.upcast_ref()] {
            widget.set_visible(!selected.is_empty());
        }
        self.empty.set_visible(selected.is_empty());
        self.details.set_visible(one.is_some());
        self.open.set_visible(one.is_some());
        self.preview.set_visible(false);
        while let Some(child) = self.art.first_child() {
            self.art.remove(&child);
        }
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        if selected.len() > 1 {
            self.art.append(&icons::icon(&selected[0], 92, 92, &self.thumbnails));
            self.name.set_label(&format!("{} items", selected.len()));
            let bytes: u64 = selected.iter().filter(|entry| !entry.is_dir).map(|entry| entry.size).sum();
            self.kind.set_label(&format!("{} in files", entry::size(bytes)));
            return;
        }
        let Some(entry) = one else { return };
        let (width, height) = if entry.is_dir { (112, 92) } else if entry.is_image() { (200, 160) } else { (92, 92) };
        self.art.append(&icons::icon(entry, width, height, &self.thumbnails));
        self.name.set_label(&entry.name);
        self.kind.set_label(&if entry.is_dir {
            match entry.children() {
                Some(1) => "Folder, 1 item".to_owned(),
                Some(n) => format!("Folder, {n} items"),
                None => "Folder".to_owned(),
            }
        } else {
            format!("{}, {}", entry.kind(), entry::size(entry.size))
        });

        let mut rows = vec![("Modified", entry::date(entry.modified))];
        match &entry.trashed {
            Some(trashed) => {
                rows.push(("From", entry::tilde(trashed.original.parent().unwrap_or(&trashed.original))));
                rows.push(("Deleted", trashed.deleted.replace('T', " ")));
            }
            None => rows.push(("Where", entry::tilde(entry.path.parent().unwrap_or(&entry.path)))),
        }
        for (row, (label, value)) in rows.into_iter().enumerate() {
            let label = gtk::Label::builder().label(label).xalign(0.0).build();
            label.add_css_class("label");
            let value = gtk::Label::builder().label(value).xalign(1.0).hexpand(true).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).max_width_chars(18).justify(gtk::Justification::Right).build();
            self.details.attach(&label, 0, row as i32, 1, 1);
            self.details.attach(&value, 1, row as i32, 1, 1);
        }
        self.open.set_label(if entry.trashed.is_some() { "Put Back" } else { "Open" });
        if entry.is_text()
            && let Some(text) = first_lines(entry)
        {
            self.preview.set_label(&text);
            self.preview.set_visible(true);
        }
    }
}

/// The start of a text file, or `None` if it isn't readable text.
fn first_lines(entry: &Entry) -> Option<String> {
    let mut buffer = vec![0; PREVIEW_BYTES];
    let read = std::fs::File::open(&entry.path).and_then(|mut file| file.read(&mut buffer)).ok()?;
    buffer.truncate(read);
    let text = String::from_utf8_lossy(&buffer);
    if text.contains('\0') {
        return None;
    }
    let lines: Vec<&str> = text.lines().take(PREVIEW_LINES).collect();
    let joined = lines.join("\n");
    (!joined.trim().is_empty()).then_some(joined)
}
