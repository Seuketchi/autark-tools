//! Kith's face, the Visor, from the design's `Persona` artboard: dark glass with glowing sage eyes, drawn
//! with cairo in a 120 × 120 box scaled to the widget. It blinks every few seconds when idle; listening
//! adds pulsing rings and wider eyes, thinking looks up with three dots, and speaking moves a mouth. With
//! reduced motion on it holds still. Frames are drawn only while something moves, so an idle face costs a
//! blink's worth of frames every few seconds.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Idle,
    Listening,
    Thinking,
    Speaking,
}

impl State {
    /// `autark-ai`'s `State` in words: idle, thinking or speaking.
    pub fn from_word(word: &str) -> Self {
        match word {
            "listening" => State::Listening,
            "thinking" => State::Thinking,
            "speaking" => State::Speaking,
            _ => State::Idle,
        }
    }
}

const GLASS: &str = "#24221E";
const EYE: &str = "#7FC1A1";
/// The face inverted for a dark background, where dark glass would vanish: a pale plate with deep sage eyes.
const GLASS_ON_DARK: &str = "#ECE8E1";
const EYE_ON_DARK: &str = "#2F6B52";
/// Seconds since the first call, which is remembered in `start`. (Setting the copy `Cell::get` returns would forget it,
/// so the time would read 0 for ever and a face that should be still would redraw on every frame.)
fn elapsed(start: &Cell<Option<f64>>, seconds: f64) -> f64 {
    let first = start.get().unwrap_or_else(|| {
        start.set(Some(seconds));
        seconds
    });
    seconds - first
}

/// Rings and dots: the design's sage, lighter on a dark background.
const SAGE: &str = "#3F7A5F";

const BLINK: f64 = 4.2;
const PULSE: f64 = 1.6;
const DOTS: f64 = 1.2;
const TALK: f64 = 0.35;
/// How long the eyes take to look up, or back.
const GLANCE: f64 = 0.3;

struct Inner {
    state: Cell<State>,
    blinks: Cell<bool>,
    pulse: Cell<bool>,
    /// When the frame clock started, in seconds, so every face moves from its own start.
    start: Cell<Option<f64>>,
    /// How far the eyes have looked up, 0 to 1, and when that last changed.
    glance: Cell<(f64, f64)>,
    ticking: Cell<Option<gtk::TickCallbackId>>,
    /// The timeout that starts the next blink while idle.
    next_blink: Cell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct Visor {
    area: gtk::DrawingArea,
    inner: Rc<Inner>,
}

impl Visor {
    pub fn new(size: i32) -> Self {
        let area = gtk::DrawingArea::builder().content_width(size).content_height(size).valign(gtk::Align::Center).halign(gtk::Align::Center).build();
        area.add_css_class("visor");
        area.update_property(&[gtk::accessible::Property::Label("Kith")]);
        let inner = Rc::new(Inner {
            state: Cell::new(State::Idle),
            blinks: Cell::new(true),
            pulse: Cell::new(false),
            start: Cell::new(None),
            glance: Cell::new((0.0, 0.0)),
            ticking: Cell::new(None),
            next_blink: Cell::new(None),
        });
        let visor = Self { area, inner };
        let drawn = visor.clone();
        visor.area.set_draw_func(move |area, cr, width, height| drawn.draw(area, cr, width, height));
        crate::theme::redraw_on_change(&visor.area);
        let mapped = visor.clone();
        visor.area.connect_map(move |_| mapped.animate());
        let unmapped = visor.clone();
        visor.area.connect_unmap(move |_| unmapped.stop());
        visor
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn state(&self) -> State {
        self.inner.state.get()
    }

    pub fn set_state(&self, state: State) {
        let old = self.inner.state.replace(state);
        if old == state {
            return;
        }
        let now = self.now();
        let (glance, since) = self.inner.glance.get();
        // Carry on from where the eyes are, so a glance cut short doesn't jump.
        let at = glance_at(glance, since, now, old == State::Thinking);
        self.inner.glance.set((at, now));
        self.area.update_property(&[gtk::accessible::Property::Description(match state {
            State::Idle => "idle",
            State::Listening => "listening",
            State::Thinking => "thinking",
            State::Speaking => "speaking",
        })]);
        self.animate();
        self.area.queue_draw();
    }

    /// Whether it blinks when idle; the bar's small face doesn't.
    pub fn set_blinks(&self, blinks: bool) {
        self.inner.blinks.set(blinks);
        self.animate();
    }

    /// Pulsing rings whatever the state, for the bar while Kith works.
    pub fn set_pulse(&self, pulse: bool) {
        if self.inner.pulse.replace(pulse) != pulse {
            self.animate();
            self.area.queue_draw();
        }
    }

    fn still() -> bool {
        crate::theme::look().theme.reduce_motion || gtk::Settings::default().is_some_and(|settings| !settings.is_gtk_enable_animations())
    }

    /// Seconds on the frame clock since this face started.
    fn now(&self) -> f64 {
        let Some(clock) = self.area.frame_clock() else { return 0.0 };
        elapsed(&self.inner.start, clock.frame_time() as f64 / 1_000_000.0)
    }

    /// Whether anything moves now, apart from blinking.
    fn moving(&self) -> bool {
        let (_, since) = self.inner.glance.get();
        self.inner.state.get() != State::Idle || self.inner.pulse.get() || self.now() - since < GLANCE
    }

    /// Ticks while something moves; while idle, only through each blink.
    fn animate(&self) {
        if !self.area.is_mapped() || Self::still() {
            self.stop();
            self.area.queue_draw();
            return;
        }
        if let Some(source) = self.inner.next_blink.take() {
            source.remove();
        }
        if self.moving() || (self.inner.blinks.get() && blink(self.now()) < 1.0) {
            self.tick();
            return;
        }
        self.stop_ticking();
        if self.inner.blinks.get() {
            // The next blink starts 90% of the way through its cycle.
            let wait = (BLINK * 0.9 - self.now().rem_euclid(BLINK)).rem_euclid(BLINK);
            let visor = self.clone();
            let source = glib::timeout_add_local_once(Duration::from_secs_f64(wait + 0.01), move || {
                visor.inner.next_blink.set(None);
                visor.tick();
            });
            self.inner.next_blink.set(Some(source));
        }
    }

    fn tick(&self) {
        if let Some(tick) = self.inner.ticking.take() {
            self.inner.ticking.set(Some(tick));
            return;
        }
        let visor = self.clone();
        let id = self.area.add_tick_callback(move |area, _| {
            area.queue_draw();
            let now = visor.now();
            if visor.moving() || (visor.inner.blinks.get() && blink(now) < 1.0) {
                return glib::ControlFlow::Continue;
            }
            visor.inner.ticking.set(None);
            visor.animate();
            glib::ControlFlow::Break
        });
        self.inner.ticking.set(Some(id));
    }

    fn stop_ticking(&self) {
        if let Some(tick) = self.inner.ticking.take() {
            tick.remove();
        }
    }

    fn stop(&self) {
        self.stop_ticking();
        if let Some(source) = self.inner.next_blink.take() {
            source.remove();
        }
    }

    fn draw(&self, _area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let still = Self::still();
        let now = if still { 0.0 } else { self.now() };
        let state = self.inner.state.get();
        let size = f64::from(width.min(height));
        cr.translate((f64::from(width) - size) / 2.0, (f64::from(height) - size) / 2.0);
        cr.scale(size / 120.0, size / 120.0);
        let dark = crate::theme::look().dark;
        let sage = if dark { EYE } else { SAGE };
        let (glass, eye) = if dark { (GLASS_ON_DARK, EYE_ON_DARK) } else { (GLASS, EYE) };

        if state == State::Listening || self.inner.pulse.get() {
            for delay in [0.0, PULSE / 2.0] {
                let phase = if still { 0.5 } else { ((now - delay).rem_euclid(PULSE)) / PULSE };
                let eased = 1.0 - (1.0 - phase).powi(3);
                let radius = 50.0 * (0.85 + 0.3 * eased);
                crate::theme::set_source(cr, sage, 0.55 * (1.0 - eased));
                // At least a pixel wide, so the bar's small face still shows its pulse.
                cr.set_line_width(1.5_f64.max(120.0 / size));
                cr.arc(60.0, 60.0, radius, 0.0, std::f64::consts::TAU);
                let _ = cr.stroke();
            }
        }

        crate::theme::set_source(cr, glass, 1.0);
        rounded_rect(cr, 14.0, 20.0, 92.0, 80.0, 30.0);
        let _ = cr.fill();

        let (glance, since) = self.inner.glance.get();
        let up = if still { if state == State::Thinking { 1.0 } else { 0.0 } } else { glance_at(glance, since, now, state == State::Thinking) };
        let (dx, dy) = (4.0 * up, -5.0 * up);
        let squeeze = if self.inner.blinks.get() && state == State::Idle && !still { blink(now) } else { 1.0 };
        let ry = if state == State::Listening { 12.0 * 1.2 } else { 12.0 } * squeeze;
        crate::theme::set_source(cr, eye, 1.0);
        for x in [44.0, 76.0] {
            ellipse(cr, x + dx, 58.0 + dy, 7.0, ry);
            let _ = cr.fill();
        }

        if state == State::Speaking {
            let phase = if still { 0.25 } else { now.rem_euclid(TALK) / TALK };
            let scale = 0.6 + 1.2 * (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos());
            let height = 3.5 * scale;
            rounded_rect(cr, 53.0, 80.0 + 1.75 - height / 2.0, 14.0, height, 1.75_f64.min(height / 2.0));
            let _ = cr.fill();
        }

        if state == State::Thinking {
            for (n, (x, y)) in [(84.0, 24.0), (93.0, 17.0), (102.0, 10.0)].into_iter().enumerate() {
                let phase = if still { 0.5 } else { (now - 0.2 * n as f64).rem_euclid(DOTS) / DOTS };
                let opacity = 0.25 + 0.75 * (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos());
                crate::theme::set_source(cr, sage, opacity);
                cr.arc(x, y, 3.0, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        }
    }
}

/// How open the eyes are at `now`: shut to 12% at 95% of each cycle, open the rest of it.
fn blink(now: f64) -> f64 {
    let phase = now.rem_euclid(BLINK) / BLINK;
    if phase < 0.9 {
        1.0
    } else if phase < 0.95 {
        1.0 - 0.88 * (phase - 0.9) / 0.05
    } else {
        0.12 + 0.88 * (phase - 0.95) / 0.05
    }
}

/// How far the eyes have looked up at `now`, easing toward `up` from `from` since `since`.
fn glance_at(from: f64, since: f64, now: f64, up: bool) -> f64 {
    let target = if up { 1.0 } else { 0.0 };
    let t = ((now - since) / GLANCE).clamp(0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    from + (target - from) * eased
}

fn ellipse(cr: &gtk::cairo::Context, x: f64, y: f64, rx: f64, ry: f64) {
    cr.save().ok();
    cr.translate(x, y);
    cr.scale(rx, ry.max(0.01));
    cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
    cr.restore().ok();
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_runs_from_the_first_look() {
        let start = Cell::new(None);
        assert_eq!(elapsed(&start, 100.0), 0.0);
        assert_eq!(elapsed(&start, 100.5), 0.5);
        assert_eq!(elapsed(&start, 103.0), 3.0);
    }

    #[test]
    fn a_blink_shuts_the_eyes_briefly() {
        assert_eq!(blink(0.0), 1.0);
        assert_eq!(blink(BLINK * 0.5), 1.0);
        assert!((blink(BLINK * 0.95) - 0.12).abs() < 1e-9);
        assert!(blink(BLINK * 0.93) < 1.0 && blink(BLINK * 0.93) > 0.12);
    }

    #[test]
    fn a_glance_eases_and_can_turn_back() {
        assert_eq!(glance_at(0.0, 0.0, 0.0, true), 0.0);
        assert_eq!(glance_at(0.0, 0.0, GLANCE, true), 1.0);
        assert!((glance_at(0.0, 0.0, GLANCE / 2.0, true) - 0.5).abs() < 1e-9);
        assert_eq!(glance_at(0.5, 1.0, 1.0 + GLANCE, false), 0.0);
    }
}
