//! The Loop logo, drawn from the design's path so it stays sharp at any scale.

use gtk::cairo::{LineCap, LineJoin};
use gtk::prelude::*;

pub fn new() -> gtk::Box {
    let area = gtk::DrawingArea::builder()
        .content_width(18)
        .content_height(18)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(|area, cr, width, height| {
        // The path is in the design's 24×24 box.
        let scale = f64::from(width.min(height)) / 24.0;
        cr.scale(scale, scale);
        cr.move_to(5.5, 19.5);
        cr.line_to(12.0, 4.5);
        cr.line_to(18.5, 19.5);
        cr.curve_to(14.5, 16.0, 9.5, 16.0, 5.5, 19.5);
        cr.close_path();
        cr.set_source_color(&area.color());
        cr.set_line_width(2.4);
        cr.set_line_cap(LineCap::Round);
        cr.set_line_join(LineJoin::Round);
        let _ = cr.stroke();
    });

    let item = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    item.add_css_class("item");
    item.add_css_class("logo");
    item.set_tooltip_text(Some("Autark"));
    item.append(&area);
    item
}
