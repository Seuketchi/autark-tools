//! What the splash looks like in each state (docs/specs/boot-splash.md): the Loop mark and the name in the middle, then the PIN
//! box or a loading bar under them. Sizes are the design's, drawn at 1440 × 900 and scaled to the screen.

use super::canvas::{Canvas, Color};
use super::font::Font;

pub const SKY: Color = 0x1E1C18;
pub const SAGE: Color = 0x4F7561;
pub const INK: Color = 0xECE8E1;
pub const MUTED: Color = 0xB5AEA3;

/// The pulse's period, in milliseconds.
const PULSE_MS: f32 = 2400.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Waiting for the disk or the system: the mark pulses.
    Starting,
    /// Asking for a secret: the box, one dot per key typed.
    Asking { dots: usize },
    /// A secret has been handed over and the answer isn't back: the box dims.
    Checking { dots: usize },
    /// After unlock, until systemd takes over: the loading bar (only once it has taken a while).
    Loading { bar: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scene {
    pub mode: Mode,
    /// The line under the box (or under the name when there is no box).
    pub message: String,
}

pub struct Fonts {
    pub regular: Font,
    pub semibold: Font,
}

impl Fonts {
    pub fn load() -> Option<Fonts> {
        Some(Fonts { regular: Font::parse(super::font::REGULAR)?, semibold: Font::parse(super::font::SEMIBOLD)? })
    }
}

/// The mark's outline in its 24 × 24 design box: an A drawn as one closed line.
fn mark_outline() -> Vec<(f32, f32)> {
    let mut points = vec![(5.5, 19.5), (12.0, 4.5), (18.5, 19.5)];
    let (p0, p1, p2, p3) = ((18.5f32, 19.5f32), (14.5f32, 16.0f32), (9.5f32, 16.0f32), (5.5f32, 19.5f32));
    for i in 1..24 {
        let t = i as f32 / 24.0;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        points.push((a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1));
    }
    points
}

/// How opaque the mark is at `ms` into the pulse: 0.55 up to 1 and back.
pub fn pulse(ms: u64) -> f32 {
    let phase = (ms as f32 % PULSE_MS) / PULSE_MS;
    0.55 + 0.45 * (0.5 - 0.5 * (phase * std::f32::consts::TAU).cos())
}

pub fn draw(canvas: &mut Canvas, fonts: &Fonts, scene: &Scene, ms: u64) {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let s = (w / 1440.0).min(h / 900.0);
    canvas.clear(SKY);
    let cx = w / 2.0;

    // The mark, 72 px in the design, with the name under it.
    let pulsing = matches!(scene.mode, Mode::Starting | Mode::Checking { .. } | Mode::Loading { .. });
    let mark_alpha = if pulsing { pulse(ms) } else { 1.0 };
    let (size, top) = (72.0 * s, h * 0.40 - 72.0 * s);
    let unit = size / 24.0;
    let outline: Vec<(f32, f32)> = mark_outline().iter().map(|&(x, y)| (cx - size / 2.0 + x * unit, top + y * unit)).collect();
    canvas.stroke_closed(&outline, 2.4 * unit, SAGE, mark_alpha);
    let name = "Autark";
    let name_px = 28.0 * s;
    let name_y = top + size + 14.0 * s + name_px * 0.75;
    fonts.semibold.draw(canvas, name, cx - fonts.semibold.width(name, name_px) / 2.0, name_y, name_px, INK, 1.0);

    // The box, or the bar, below.
    let (box_w, box_h) = (260.0 * s, 40.0 * s);
    let box_y = h * 0.62;
    let message_px = 14.0 * s;
    let mut message_y = box_y;
    match scene.mode {
        Mode::Asking { dots } | Mode::Checking { dots } => {
            let dim = if matches!(scene.mode, Mode::Checking { .. }) { 0.5 } else { 1.0 };
            canvas.round_rect(cx - box_w / 2.0, box_y, box_w, box_h, box_h / 2.0, INK, 0.14 * dim);
            let (radius, pitch, pad) = (4.0 * s, 16.0 * s, 20.0 * s);
            let room = ((box_w - 2.0 * pad) / pitch).floor() as usize;
            for i in 0..dots.min(room) {
                canvas.circle(cx - box_w / 2.0 + pad + radius + i as f32 * pitch, box_y + box_h / 2.0, radius, INK, dim);
            }
            message_y = box_y + box_h + 14.0 * s;
        }
        Mode::Loading { bar: true } => {
            let (bar_w, bar_h) = (160.0 * s, 4.0 * s);
            let x = cx - bar_w / 2.0;
            canvas.round_rect(x, box_y + box_h / 2.0, bar_w, bar_h, bar_h / 2.0, INK, 0.14);
            let travel = (ms % 1600) as f32 / 1600.0;
            let (seg_w, seg_x) = (bar_w * 0.4, x + (bar_w * 1.4) * travel - bar_w * 0.4);
            let (left, right) = (seg_x.max(x), (seg_x + seg_w).min(x + bar_w));
            if right > left {
                canvas.round_rect(left, box_y + box_h / 2.0, right - left, bar_h, bar_h / 2.0, SAGE, 1.0);
            }
        }
        _ => {}
    }
    if !scene.message.is_empty() {
        let text_w = fonts.regular.width(&scene.message, message_px);
        fonts.regular.draw(canvas, &scene.message, cx - text_w / 2.0, message_y + message_px * 0.8, message_px, MUTED, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(mode: Mode, message: &str, ms: u64) -> Canvas {
        let mut canvas = Canvas::new(1280, 800);
        draw(&mut canvas, &Fonts::load().unwrap(), &Scene { mode, message: message.to_owned() }, ms);
        canvas
    }

    fn lit(canvas: &Canvas, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| canvas.at(x, y) != SKY).count()
    }

    #[test]
    fn the_pulse_stays_between_its_two_opacities_and_repeats() {
        for ms in (0..5000).step_by(37) {
            let p = pulse(ms);
            assert!((0.55..=1.0).contains(&p), "{p} at {ms}");
        }
        assert!((pulse(0) - 0.55).abs() < 0.01);
        assert!((pulse(1200) - 1.0).abs() < 0.01);
        assert_eq!(pulse(100), pulse(100 + 2400));
    }

    #[test]
    fn starting_shows_the_mark_and_the_name_and_nothing_below() {
        let canvas = render(Mode::Starting, "", 0);
        assert!(lit(&canvas, 560, 200, 720, 320) > 500, "no mark");
        assert!(lit(&canvas, 560, 330, 720, 400) > 100, "no name");
        assert_eq!(lit(&canvas, 0, 480, 1280, 800), 0, "something drawn where the box goes");
    }

    #[test]
    fn asking_draws_the_box_with_a_dot_for_each_key() {
        let none = render(Mode::Asking { dots: 0 }, "Enter your disk PIN", 0);
        let four = render(Mode::Asking { dots: 4 }, "Enter your disk PIN", 0);
        assert!(lit(&none, 480, 490, 800, 560) > 5000, "no box");
        let ink = |c: &Canvas| (490..560).flat_map(|y| (480..800).map(move |x| (x, y))).filter(|&(x, y)| c.at(x, y) > 0x808080).count();
        assert!(ink(&four) > ink(&none) + 100, "typing didn't add dots");
        assert!(lit(&four, 400, 536, 880, 566) > 100, "no message under the box");
    }

    #[test]
    fn dots_stop_at_the_edge_of_the_box() {
        let many = render(Mode::Asking { dots: 500 }, "", 0);
        // The box is 260 × 1280/1440 wide, centred: nothing may be bright outside it.
        let (left, right) = (640 - 116, 640 + 116);
        let bright_outside = (490..560).flat_map(|y| (0..1280).map(move |x| (x, y))).filter(|&(x, y)| (x < left - 2 || x > right + 2) && many.at(x, y) > 0x808080).count();
        assert_eq!(bright_outside, 0);
    }

    #[test]
    fn checking_dims_the_box() {
        let asking = render(Mode::Asking { dots: 3 }, "", 0);
        let checking = render(Mode::Checking { dots: 3 }, "", 1200);
        let brightest = |c: &Canvas| (490..560).flat_map(|y| (480..800).map(move |x| c.at(x, y) & 0xff)).max().unwrap();
        assert!(brightest(&checking) < brightest(&asking) - 40);
    }

    #[test]
    fn the_loading_bar_only_shows_when_asked_for() {
        assert_eq!(lit(&render(Mode::Loading { bar: false }, "", 0), 0, 480, 1280, 800), 0);
        assert!(lit(&render(Mode::Loading { bar: true }, "", 800), 0, 480, 1280, 800) > 100);
    }

    #[test]
    fn a_small_screen_still_draws_inside_itself() {
        let mut canvas = Canvas::new(640, 480);
        draw(&mut canvas, &Fonts::load().unwrap(), &Scene { mode: Mode::Asking { dots: 6 }, message: "Enter your disk PIN".into() }, 0);
        assert!(lit(&canvas, 0, 0, 640, 480) > 1000);
    }
}
