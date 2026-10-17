//! The design's icons, drawn: a two-tone folder, and a page with the file's extension on a
//! badge. Images show their thumbnail instead.

use std::f64::consts::PI;

use gtk::cairo::Context;
use gtk::prelude::*;

use crate::entry::Entry;
use crate::thumbnails::Thumbnails;

/// A widget `width` × `height` showing the entry: its thumbnail if it's an image and one can be
/// made, otherwise the drawn icon.
pub fn icon(entry: &Entry, width: i32, height: i32, thumbnails: &Thumbnails) -> gtk::Widget {
    let drawn: gtk::Widget = if entry.is_dir { folder(width, height).upcast() } else { document(entry, width, height) };
    if !entry.is_image() {
        return drawn;
    }
    let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    holder.set_halign(gtk::Align::Center);
    holder.append(&drawn);
    let weak = holder.downgrade();
    thumbnails.request(entry, move |surface| {
        let Some(holder) = weak.upgrade() else { return };
        // Drawn scaled into the icon's box, so a big image never makes the tile bigger.
        let picture = area(width, height);
        picture.set_draw_func(move |_, cr, width, height| {
            let (w, h) = (f64::from(surface.width()), f64::from(surface.height()));
            let scale = (f64::from(width) / w).min(f64::from(height) / h);
            cr.translate((f64::from(width) - w * scale) / 2.0, (f64::from(height) - h * scale) / 2.0);
            cr.scale(scale, scale);
            let _ = cr.set_source_surface(&surface, 0.0, 0.0);
            cr.source().set_filter(gtk::cairo::Filter::Good);
            let _ = cr.paint();
        });
        while let Some(child) = holder.first_child() {
            holder.remove(&child);
        }
        holder.append(&picture);
    });
    holder.upcast()
}

fn area(width: i32, height: i32) -> gtk::DrawingArea {
    gtk::DrawingArea::builder().content_width(width).content_height(height).halign(gtk::Align::Center).build()
}

fn rgb(cr: &Context, hex: u32) {
    cr.set_source_rgb(f64::from(hex >> 16 & 0xff) / 255.0, f64::from(hex >> 8 & 0xff) / 255.0, f64::from(hex & 0xff) / 255.0);
}

/// Fits a `w` × `h` design box into the area, centred.
fn fit(cr: &Context, width: i32, height: i32, w: f64, h: f64) {
    let scale = (f64::from(width) / w).min(f64::from(height) / h);
    cr.translate((f64::from(width) - w * scale) / 2.0, (f64::from(height) - h * scale) / 2.0);
    cr.scale(scale, scale);
}

pub fn folder(width: i32, height: i32) -> gtk::DrawingArea {
    let area = area(width, height);
    area.set_draw_func(|_, cr, width, height| draw_folder(cr, width, height));
    area
}

fn draw_folder(cr: &Context, width: i32, height: i32) {
    fit(cr, width, height, 56.0, 46.0);
    // The back, with its tab.
    cr.move_to(6.0, 4.0);
    cr.line_to(20.0, 4.0);
    cr.line_to(25.0, 9.0);
    cr.line_to(50.0, 9.0);
    cr.arc(50.0, 13.0, 4.0, -PI / 2.0, 0.0);
    cr.line_to(54.0, 40.0);
    cr.arc(50.0, 40.0, 4.0, 0.0, PI / 2.0);
    cr.line_to(6.0, 44.0);
    cr.arc(6.0, 40.0, 4.0, PI / 2.0, PI);
    cr.line_to(2.0, 8.0);
    cr.arc(6.0, 8.0, 4.0, PI, 1.5 * PI);
    rgb(cr, 0x336650);
    let _ = cr.fill();
    // The front.
    cr.move_to(2.0, 15.0);
    cr.line_to(54.0, 15.0);
    cr.line_to(54.0, 40.0);
    cr.arc(50.0, 40.0, 4.0, 0.0, PI / 2.0);
    cr.line_to(6.0, 44.0);
    cr.arc(6.0, 40.0, 4.0, PI / 2.0, PI);
    cr.close_path();
    rgb(cr, 0x3F7A5F);
    let _ = cr.fill();
}

fn draw_page(cr: &Context, width: i32, height: i32, lines: bool) {
    fit(cr, width, height, 46.0, 46.0);
    cr.move_to(10.0, 2.0);
    cr.line_to(28.0, 2.0);
    cr.line_to(38.0, 12.0);
    cr.line_to(38.0, 42.0);
    cr.arc(36.0, 42.0, 2.0, 0.0, PI / 2.0);
    cr.line_to(10.0, 44.0);
    cr.arc(10.0, 42.0, 2.0, PI / 2.0, PI);
    cr.line_to(8.0, 4.0);
    cr.arc(10.0, 4.0, 2.0, PI, 1.5 * PI);
    rgb(cr, 0xE9ECF1);
    let _ = cr.fill();
    cr.move_to(28.0, 2.0);
    cr.line_to(28.0, 12.0);
    cr.line_to(38.0, 12.0);
    cr.close_path();
    rgb(cr, 0xC3CAD4);
    let _ = cr.fill();
    if lines {
        rgb(cr, 0x7A736A);
        cr.set_line_width(1.6);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        for (y, end) in [(20.0, 32.0), (25.0, 32.0), (30.0, 26.0)] {
            cr.move_to(14.0, y);
            cr.line_to(end, y);
        }
        let _ = cr.stroke();
    }
}

fn document(entry: &Entry, width: i32, height: i32) -> gtk::Widget {
    let page = area(width, height);
    let badge = entry.badge();
    // The big preview has no badge but ruled lines, as in the design.
    let large = height > 80;
    page.set_draw_func(move |_, cr, width, height| draw_page(cr, width, height, large));
    if badge.is_empty() || large {
        return page.upcast();
    }
    let label = gtk::Label::new(Some(&badge));
    label.add_css_class("badge");
    label.set_halign(gtk::Align::Center);
    label.set_valign(gtk::Align::End);
    label.set_margin_bottom(height / 10);
    let overlay = gtk::Overlay::builder().child(&page).build();
    overlay.add_overlay(&label);
    overlay.upcast()
}

/// The list view's 16-pixel icon.
pub fn mini(entry: &Entry) -> gtk::DrawingArea {
    let area = area(16, 16);
    let is_dir = entry.is_dir;
    area.set_draw_func(move |_, cr, width, height| if is_dir { draw_folder(cr, width, height) } else { draw_page(cr, width, height, false) });
    area
}
