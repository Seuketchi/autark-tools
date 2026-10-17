//! The sidebar: favourite folders, then Locations (mounted disks and the Trash).

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use gtk::cairo::Context;
use gtk::prelude::*;
use gtk::glib;

use crate::state;

#[derive(Clone, Copy)]
pub enum Glyph {
    Home,
    Desktop,
    Documents,
    Downloads,
    Folder,
    Drive,
    Trash,
}

pub fn glyph_for(path: &Path) -> Glyph {
    let special = |dir| glib::user_special_dir(dir).is_some_and(|special| special == path);
    if path == glib::home_dir() {
        Glyph::Home
    } else if special(glib::UserDirectory::Desktop) {
        Glyph::Desktop
    } else if special(glib::UserDirectory::Documents) {
        Glyph::Documents
    } else if special(glib::UserDirectory::Downloads) {
        Glyph::Downloads
    } else {
        Glyph::Folder
    }
}

/// A 16-pixel line glyph in the accent colour.
pub fn glyph(kind: Glyph) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(16).content_height(16).valign(gtk::Align::Center).build();
    area.add_css_class("glyph");
    area.set_draw_func(move |area, cr, width, _| {
        cr.scale(f64::from(width) / 24.0, f64::from(width) / 24.0);
        cr.set_source_color(&area.color());
        cr.set_line_width(2.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        draw(cr, kind);
        let _ = cr.stroke();
    });
    area
}

fn polyline(cr: &Context, points: &[(f64, f64)]) {
    cr.move_to(points[0].0, points[0].1);
    for &(x, y) in &points[1..] {
        cr.line_to(x, y);
    }
}

fn draw(cr: &Context, kind: Glyph) {
    match kind {
        Glyph::Home => {
            polyline(cr, &[(3.0, 11.0), (12.0, 4.0), (21.0, 11.0)]);
            polyline(cr, &[(5.0, 10.0), (5.0, 20.0), (19.0, 20.0), (19.0, 10.0)]);
            polyline(cr, &[(10.0, 20.0), (10.0, 14.0), (14.0, 14.0), (14.0, 20.0)]);
        }
        Glyph::Desktop => {
            cr.rectangle(3.0, 4.0, 18.0, 12.0);
            polyline(cr, &[(8.0, 20.0), (16.0, 20.0)]);
            polyline(cr, &[(12.0, 16.0), (12.0, 20.0)]);
        }
        Glyph::Documents => {
            polyline(cr, &[(14.0, 3.0), (6.0, 3.0), (6.0, 21.0), (18.0, 21.0), (18.0, 7.0), (14.0, 3.0), (14.0, 7.0), (18.0, 7.0)]);
        }
        Glyph::Downloads => {
            polyline(cr, &[(12.0, 4.0), (12.0, 15.0)]);
            polyline(cr, &[(7.0, 10.0), (12.0, 15.0), (17.0, 10.0)]);
            polyline(cr, &[(5.0, 20.0), (19.0, 20.0)]);
        }
        Glyph::Folder => {
            polyline(cr, &[(3.0, 6.0), (9.0, 6.0), (11.0, 8.0), (21.0, 8.0), (21.0, 19.0), (3.0, 19.0), (3.0, 6.0)]);
        }
        Glyph::Drive => {
            cr.rectangle(3.0, 14.0, 18.0, 5.0);
            polyline(cr, &[(5.0, 14.0), (7.0, 5.0), (17.0, 5.0), (19.0, 14.0)]);
            cr.new_sub_path();
            cr.arc(7.0, 16.5, 0.3, 0.0, 2.0 * PI);
        }
        Glyph::Trash => {
            polyline(cr, &[(4.0, 7.0), (20.0, 7.0)]);
            polyline(cr, &[(9.0, 7.0), (9.0, 4.0), (15.0, 4.0), (15.0, 7.0)]);
            polyline(cr, &[(6.0, 7.0), (7.0, 20.0), (17.0, 20.0), (18.0, 7.0)]);
        }
    }
}

/// One row: glyph, name, and an optional note at the end ("ext4").
pub fn row(kind: Glyph, name: &str, note: Option<&str>) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.append(&glyph(kind));
    let label = gtk::Label::builder().label(name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
    content.append(&label);
    if let Some(note) = note {
        let note = gtk::Label::new(Some(note));
        note.add_css_class("note");
        content.append(&note);
    }
    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("place");
    button
}

pub fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("heading");
    label
}

/// The favourites as places, skipping folders that are gone.
pub fn favourites(saved: &[String]) -> Vec<(String, PathBuf)> {
    saved
        .iter()
        .map(|saved| state::expand(saved))
        .filter(|path| path.is_dir())
        .map(|path| {
            let name = if path == glib::home_dir() { "Home".to_owned() } else { path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned()) };
            (name, path)
        })
        .collect()
}
