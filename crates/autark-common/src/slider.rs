//! A thin horizontal slider drawn in code, as the player's seek bar and volume and the hardware sliders are: a track, the part before the value
//! in the accent, and a knob. GTK's own scale is styled by theme rules that are hard to get under.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

type Changed = RefCell<Option<Box<dyn Fn(f64)>>>;

pub struct Slider {
    pub widget: gtk::DrawingArea,
    value: Cell<f64>,
    dragging: Cell<bool>,
    changed: Changed,
}

const TRACK: f64 = 4.0;
const KNOB: f64 = 7.0;

impl Slider {
    /// `value` is 0 to 1.
    pub fn new(value: f64) -> Rc<Self> {
        let widget = gtk::DrawingArea::builder().height_request(20).valign(gtk::Align::Center).build();
        widget.add_css_class("slider");
        let slider = Rc::new(Self { widget, value: Cell::new(value.clamp(0.0, 1.0)), dragging: Cell::new(false), changed: RefCell::default() });
        slider.widget.set_draw_func({
            let slider = Rc::downgrade(&slider);
            move |_, cr, width, height| {
                if let Some(slider) = slider.upgrade() {
                    slider.draw(cr, f64::from(width), f64::from(height));
                }
            }
        });
        crate::theme::redraw_on_change(&slider.widget);

        // Pressing sets the value where it lands, and dragging carries on from there.
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin({
            let slider = Rc::downgrade(&slider);
            move |_, x, _| {
                if let Some(slider) = slider.upgrade() {
                    slider.dragging.set(true);
                    slider.user_set(x);
                }
            }
        });
        drag.connect_drag_update({
            let (slider, drag_) = (Rc::downgrade(&slider), drag.clone());
            move |_, dx, _| {
                if let (Some(slider), Some((x, _))) = (slider.upgrade(), drag_.start_point()) {
                    slider.user_set(x + dx);
                }
            }
        });
        drag.connect_drag_end({
            let slider = Rc::downgrade(&slider);
            move |_, _, _| {
                if let Some(slider) = slider.upgrade() {
                    slider.dragging.set(false);
                }
            }
        });
        slider.widget.add_controller(drag);
        slider
    }

    pub fn value(&self) -> f64 {
        self.value.get()
    }

    /// Moves the knob without telling `connect_changed`, which is for what you do.
    pub fn set_value(&self, value: f64) {
        let value = value.clamp(0.0, 1.0);
        if (value - self.value.get()).abs() > f64::EPSILON {
            self.value.set(value);
            self.widget.queue_draw();
        }
    }

    /// Whether it's being dragged, so playback doesn't pull the knob from under the pointer.
    pub fn dragging(&self) -> bool {
        self.dragging.get()
    }

    /// Calls `changed` with the new value when you press or drag.
    pub fn connect_changed(&self, changed: impl Fn(f64) + 'static) {
        *self.changed.borrow_mut() = Some(Box::new(changed));
    }

    fn user_set(&self, x: f64) {
        let room = (f64::from(self.widget.width()) - 2.0 * KNOB).max(1.0);
        let value = ((x - KNOB) / room).clamp(0.0, 1.0);
        self.set_value(value);
        if let Some(changed) = &*self.changed.borrow() {
            changed(value);
        }
    }

    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let look = crate::theme::look();
        let (x0, x1, y) = (KNOB, width - KNOB, height / 2.0);
        let x = x0 + (x1 - x0) * self.value.get();
        let track = |from: f64, to: f64| {
            cr.new_sub_path();
            cr.arc(to - TRACK / 2.0, y, TRACK / 2.0, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            cr.arc(from + TRACK / 2.0, y, TRACK / 2.0, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
            cr.close_path();
        };
        track(x0 - TRACK / 2.0, x1 + TRACK / 2.0);
        crate::theme::set_source(cr, &look.palette.text, 0.16);
        let _ = cr.fill();
        if x > x0 {
            track(x0 - TRACK / 2.0, x + TRACK / 2.0);
            crate::theme::set_source(cr, &look.palette.accent, 1.0);
            let _ = cr.fill();
        }
        cr.arc(x, y, KNOB - 1.0, 0.0, 2.0 * std::f64::consts::PI);
        crate::theme::set_source(cr, &look.palette.accent, 1.0);
        let _ = cr.fill();
    }
}
