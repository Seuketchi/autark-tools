//! The mascot: a small cream pixel cat with a lightning bolt for a tail, whose face follows the plan: happy
//! under budget, worried over it, curious while the estimate is early and asleep with nothing to go on. It is drawn
//! from a grid of squares, so there are no pictures to ship and nothing that moves, so it costs no idle CPU.

use std::cell::Cell;
use std::rc::Rc;

use autark_finance::electric::{Basis, Plan};
use gtk::cairo;
use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Sleepy,
    Curious,
    Happy,
    Worried,
}

pub fn mood_for(plan: &Plan) -> Mood {
    match plan.basis {
        Basis::None => Mood::Sleepy,
        Basis::Early | Basis::Appliances => Mood::Curious,
        Basis::Readings if plan.is_over() => Mood::Worried,
        Basis::Readings => Mood::Happy,
    }
}

/// What it says, under the picture.
pub fn line(mood: Mood) -> &'static str {
    match mood {
        Mood::Sleepy => "Log a reading and I'll wake up.",
        Mood::Curious => "Still learning your pace.",
        Mood::Happy => "Nice. You're on budget.",
        Mood::Worried => "Easy on the power!",
    }
}

const COLUMNS: f64 = 16.0;
const ROWS: f64 = 14.0;

/// `C` the cat, `T` its ears and paws in tan, `I` the inside of an ear, `Y` the bolt that is its tail. The face is
/// added per mood.
const SHAPE: [&str; 14] = [
    "...T........T...",
    "...TI......IT...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "...CCCCCCCCCC...",
    "....CCCCCCCC..YY",
    "....CCCCCCCC.YY.",
    "....CCCCCCCCYYYY",
    "....CCCCCCCC.YY.",
    "....CCCCCCCCY...",
    "....TT....TT....",
    "....TT....TT....",
];

type Colour = (f64, f64, f64);

const BODY: Colour = (0.949, 0.851, 0.702);
const TAN: Colour = (0.788, 0.604, 0.400);
const EAR: Colour = (0.918, 0.620, 0.620);
const BOLT: Colour = (1.0, 0.835, 0.290);
const FACE: Colour = (0.169, 0.102, 0.071);
const NOSE: Colour = (0.890, 0.510, 0.540);
const DROP: Colour = (0.455, 0.769, 0.949);
const SLEEP: Colour = (0.62, 0.62, 0.66);

type Pixels = &'static [(u8, u8)];
type Extras = Vec<(Pixels, Colour)>;

/// Whiskers, which every face has.
const WHISKERS: Pixels = &[(1, 5), (2, 5), (13, 5), (14, 5)];
const NOSE_PIXELS: Pixels = &[(7, 4), (8, 4)];

/// (column, row) of each dark face square, per mood, and extras in their own colour.
fn face(mood: Mood) -> (Vec<(u8, u8)>, Extras) {
    let mut dark: Vec<(u8, u8)> = WHISKERS.to_vec();
    let mut extras: Extras = vec![(NOSE_PIXELS, NOSE)];
    match mood {
        // Eyes shut in a smile, and a smile.
        Mood::Happy => dark.extend([(4, 4), (5, 3), (6, 4), (9, 4), (10, 3), (11, 4), (6, 5), (7, 6), (8, 6), (9, 5)]),
        // Wide eyes, a frown and a drop of sweat.
        Mood::Worried => {
            dark.extend([(5, 3), (5, 4), (10, 3), (10, 4), (6, 6), (7, 5), (8, 5), (9, 6)]);
            extras.push((&[(13, 2), (13, 3)], DROP));
        }
        // Wide eyes, a small mouth, and a question.
        Mood::Curious => {
            dark.extend([(5, 3), (5, 4), (10, 3), (10, 4), (7, 6), (8, 6)]);
            extras.push((&[(13, 0), (14, 0), (14, 1), (13, 2), (13, 4)], BOLT));
        }
        // Eyes shut flat, a small mouth, and a z.
        Mood::Sleepy => {
            dark.extend([(4, 4), (5, 4), (10, 4), (11, 4), (7, 6), (8, 6)]);
            extras.push((&[(13, 0), (14, 0), (15, 0), (14, 1), (13, 2), (14, 2), (15, 2)], SLEEP));
        }
    }
    (dark, extras)
}

fn square(cr: &cairo::Context, colour: (f64, f64, f64), x: f64, y: f64, cell: f64) {
    cr.set_source_rgb(colour.0, colour.1, colour.2);
    cr.rectangle(x, y, cell, cell);
    let _ = cr.fill();
}

/// Draws the creature centred in `width` by `height`, on whole pixels so the edges stay sharp.
pub fn draw(cr: &cairo::Context, width: f64, height: f64, mood: Mood) {
    let cell = (width / COLUMNS).min(height / ROWS).floor().max(1.0);
    let (left, top) = (((width - cell * COLUMNS) / 2.0).floor(), ((height - cell * ROWS) / 2.0).floor());
    let at = |column: u8, row: u8| (left + f64::from(column) * cell, top + f64::from(row) * cell);

    for (row, line) in SHAPE.iter().enumerate() {
        for (column, kind) in line.chars().enumerate() {
            let colour = match kind {
                'C' => BODY,
                'T' => TAN,
                'I' => EAR,
                'Y' => BOLT,
                _ => continue,
            };
            let (x, y) = at(column as u8, row as u8);
            square(cr, colour, x, y, cell);
        }
    }
    let (inside, outside) = face(mood);
    for (column, row) in inside {
        let (x, y) = at(column, row);
        square(cr, FACE, x, y, cell);
    }
    for (pixels, colour) in outside {
        for &(column, row) in pixels {
            let (x, y) = at(column, row);
            square(cr, colour, x, y, cell);
        }
    }
}

pub struct Mascot {
    pub area: gtk::DrawingArea,
    mood: Rc<Cell<Mood>>,
}

impl Mascot {
    pub fn new() -> Mascot {
        let mood = Rc::new(Cell::new(Mood::Sleepy));
        let area = gtk::DrawingArea::builder().content_width(112).content_height(98).halign(gtk::Align::Center).build();
        area.set_draw_func({
            let mood = mood.clone();
            move |_, cr, width, height| draw(cr, f64::from(width), f64::from(height), mood.get())
        });
        Mascot { area, mood }
    }

    pub fn set(&self, mood: Mood) {
        if self.mood.replace(mood) != mood {
            self.area.queue_draw();
        }
    }
}

#[cfg(test)]
mod tests {
    use autark_finance::electric::{Config, Reading, plan};
    use autark_finance::money::Money;
    use chrono::NaiveDate;

    use super::*;

    const MOODS: [Mood; 4] = [Mood::Sleepy, Mood::Curious, Mood::Happy, Mood::Worried];

    fn picture(mood: Mood) -> Vec<u8> {
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 112, 98).unwrap();
        draw(&cairo::Context::new(&surface).unwrap(), 112.0, 98.0, mood);
        surface.flush();
        surface.data().unwrap().to_vec()
    }

    #[test]
    fn each_mood_draws_something_and_they_differ() {
        let pictures: Vec<Vec<u8>> = MOODS.iter().map(|mood| picture(*mood)).collect();
        for (mood, pixels) in MOODS.iter().zip(&pictures) {
            assert!(pixels.iter().any(|byte| *byte != 0), "{mood:?} drew nothing");
        }
        for (a, b) in [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
            assert_ne!(pictures[a], pictures[b], "{:?} and {:?} look the same", MOODS[a], MOODS[b]);
        }
    }

    #[test]
    fn it_stays_inside_a_small_area() {
        // A 10 by 8 box is smaller than one square per cell, so it still draws, one pixel a cell, and does not panic.
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 10, 8).unwrap();
        for mood in MOODS {
            draw(&cairo::Context::new(&surface).unwrap(), 10.0, 8.0, mood);
        }
    }

    #[test]
    fn the_plan_sets_the_mood() {
        let day = |n| NaiveDate::from_ymd_opt(2026, 11, n).unwrap();
        let reading = |n, kwh| Reading { date: day(n), kwh, note: String::new() };
        let config = Config::default();
        assert_eq!(mood_for(&plan(&config, &[], &[], day(3))), Mood::Sleepy);
        assert_eq!(mood_for(&plan(&config, &[reading(1, 0.0), reading(3, 2.0)], &[], day(3))), Mood::Curious);
        let slow: Vec<_> = (0..=8).map(|n| reading(1 + n, 2.0 * f64::from(n))).collect();
        assert_eq!(mood_for(&plan(&config, &slow, &[], day(9))), Mood::Happy);
        let fast: Vec<_> = (0..=8).map(|n| reading(1 + n, 6.0 * f64::from(n))).collect();
        let tight = Config { budget: Money::from_pesos(500), ..config };
        assert_eq!(mood_for(&plan(&tight, &fast, &[], day(9))), Mood::Worried);
    }

    /// `MASCOT_PNG=/some/folder cargo test -p autark-budget dump -- --ignored` writes one picture per mood to look at.
    #[test]
    #[ignore = "writes pictures when MASCOT_PNG is set"]
    fn dump_the_faces() {
        let Some(folder) = std::env::var_os("MASCOT_PNG") else { return };
        let sheet = cairo::ImageSurface::create(cairo::Format::ARgb32, 4 * 140, 130).unwrap();
        let cr = cairo::Context::new(&sheet).unwrap();
        cr.set_source_rgb(0.97, 0.96, 0.94);
        cr.paint().unwrap();
        for (index, mood) in MOODS.iter().enumerate() {
            cr.save().unwrap();
            cr.translate(index as f64 * 140.0 + 14.0, 16.0);
            draw(&cr, 112.0, 98.0, *mood);
            cr.restore().unwrap();
        }
        // cairo's PNG writer is not built in, so write the raw pixels (BGRA) for a script to turn into a picture.
        drop(cr);
        let mut sheet = sheet;
        sheet.flush();
        std::fs::write(std::path::Path::new(&folder).join("mascot.bgra"), &*sheet.data().unwrap()).unwrap();
    }
}
