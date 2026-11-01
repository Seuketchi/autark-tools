//! A small bar chart drawn with cairo in the theme's colours: one bar per value, a dashed line for a limit, and bars
//! over the limit in the danger colour. Days that are averages rather than measurements are drawn paler.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Data {
    /// One bar each; `None` leaves a gap.
    pub values: Vec<Option<f64>>,
    /// Same length as `values`: false draws the bar paler.
    pub solid: Vec<bool>,
    /// A level to mark, and to colour bars above it.
    pub limit: Option<f64>,
}

pub struct Bars {
    pub area: gtk::DrawingArea,
    data: Rc<RefCell<Data>>,
}

impl Bars {
    pub fn new(height: i32) -> Bars {
        let data = Rc::new(RefCell::new(Data::default()));
        let area = gtk::DrawingArea::builder().content_height(height).hexpand(true).build();
        area.set_draw_func({
            let data = data.clone();
            move |_, cr, width, height| draw(cr, f64::from(width), f64::from(height), &data.borrow())
        });
        autark_common::theme::redraw_on_change(&area);
        Bars { area, data }
    }

    pub fn set(&self, data: Data) {
        if self.data.replace(data.clone()) != data {
            self.area.queue_draw();
        }
    }
}

pub fn draw(cr: &cairo::Context, width: f64, height: f64, data: &Data) {
    let look = autark_common::theme::look();
    let palette = &look.palette;
    let paint = |hex: &str, alpha: f64| autark_common::theme::set_source(cr, hex, alpha);

    // The baseline.
    paint(&palette.border, 1.0);
    cr.set_line_width(1.0);
    cr.move_to(0.0, height - 0.5);
    cr.line_to(width, height - 0.5);
    let _ = cr.stroke();

    let count = data.values.len();
    if count == 0 {
        return;
    }
    let top = data.values.iter().flatten().copied().chain(data.limit.map(|limit| limit * 1.15)).fold(0.0, f64::max);
    if top <= 0.0 {
        return;
    }
    let usable = height - 4.0;
    let gap = 4.0;
    let bar = ((width - gap * (count as f64 - 1.0)) / count as f64).floor().max(1.0);
    let used = bar * count as f64 + gap * (count as f64 - 1.0);
    let left = ((width - used) / 2.0).floor();
    for (index, value) in data.values.iter().enumerate() {
        let Some(value) = value else { continue };
        let tall = (value / top * usable).max(if *value > 0.0 { 2.0 } else { 0.0 });
        let over = data.limit.is_some_and(|limit| *value > limit);
        let alpha = if data.solid.get(index).copied().unwrap_or(true) { 1.0 } else { 0.45 };
        paint(if over { &palette.danger } else { &palette.accent }, alpha);
        let x = left + index as f64 * (bar + gap);
        cr.rectangle(x, height - 1.0 - tall, bar, tall);
        let _ = cr.fill();
    }
    if let Some(limit) = data.limit {
        let y = (height - 1.0 - limit / top * usable).round() + 0.5;
        paint(&palette.text2, 0.8);
        cr.set_dash(&[4.0, 3.0], 0.0);
        cr.move_to(0.0, y);
        cr.line_to(width, y);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
    }
}
