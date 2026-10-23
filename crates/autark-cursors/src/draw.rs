//! The shapes, drawn with cairo on a 32 by 32 grid and scaled to each size.

use std::f64::consts::PI;
use std::path::Path;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk::cairo::{Context, FontSlant, FontWeight, Format, ImageSurface, LineCap, LineJoin};

use crate::xcursor::Image;

/// The nominal sizes written; a toolkit takes the nearest to what it wants.
const SIZES: [u32; 5] = [24, 32, 48, 64, 96];
const GRID: f64 = 32.0;
/// Frames in a spinning cursor, and how long each shows.
const TURN: u32 = 12;
const FRAME_MS: u32 = 80;

/// Near-black with a hint of warmth, to sit with the Linen theme.
const INK: (f64, f64, f64) = (0.09, 0.08, 0.07);

pub struct Cursor {
    pub name: &'static str,
    /// The point that clicks, on the grid.
    hot: (f64, f64),
    /// 1 for a still cursor, `TURN` for one that spins.
    frames: u32,
    draw: fn(&Context, u32),
}

pub const CURSORS: &[Cursor] = &[
    Cursor { name: "default", hot: (6.0, 3.0), frames: 1, draw: |cr, _| solid(cr, &[&arrow]) },
    Cursor { name: "text", hot: (16.0, 16.0), frames: 1, draw: |cr, _| lines(cr, &text_beam, 4.0, 2.0) },
    Cursor { name: "vertical-text", hot: (16.0, 16.0), frames: 1, draw: |cr, _| lines(cr, &|cr| rotated(cr, PI / 2.0, &text_beam), 4.0, 2.0) },
    Cursor { name: "pointer", hot: (14.5, 4.0), frames: 1, draw: |cr, _| solid(cr, &[&|cr| rounded(cr, 12.0, 4.0, 5.0, 14.0, 2.5), &|cr| rounded(cr, 8.0, 14.0, 16.0, 13.0, 4.5)]) },
    Cursor { name: "wait", hot: (16.0, 16.0), frames: TURN, draw: |cr, frame| lines(cr, &|cr| ring(cr, 16.0, 16.0, 9.0, frame), 5.0, 3.0) },
    Cursor {
        name: "progress",
        hot: (6.0, 3.0),
        frames: TURN,
        draw: |cr, frame| {
            solid(cr, &[&arrow]);
            lines(cr, &|cr| ring(cr, 24.5, 24.5, 4.0, frame), 3.6, 1.8);
        },
    },
    Cursor { name: "crosshair", hot: (16.0, 16.0), frames: 1, draw: |cr, _| lines(cr, &crosshair, 4.0, 2.0) },
    Cursor { name: "move", hot: (16.0, 16.0), frames: 1, draw: |cr, _| solid(cr, &[&four_arrows]) },
    Cursor { name: "ew-resize", hot: (16.0, 16.0), frames: 1, draw: |cr, _| solid(cr, &[&double_arrow]) },
    Cursor { name: "ns-resize", hot: (16.0, 16.0), frames: 1, draw: |cr, _| solid(cr, &[&|cr| rotated(cr, PI / 2.0, &double_arrow)]) },
    Cursor { name: "nwse-resize", hot: (16.0, 16.0), frames: 1, draw: |cr, _| solid(cr, &[&|cr| rotated(cr, PI / 4.0, &double_arrow)]) },
    Cursor { name: "nesw-resize", hot: (16.0, 16.0), frames: 1, draw: |cr, _| solid(cr, &[&|cr| rotated(cr, -PI / 4.0, &double_arrow)]) },
    Cursor { name: "not-allowed", hot: (16.0, 16.0), frames: 1, draw: |cr, _| lines(cr, &forbidden, 5.0, 3.0) },
    Cursor {
        name: "grab",
        hot: (16.0, 16.0),
        frames: 1,
        draw: |cr, _| {
            solid(
                cr,
                &[
                    &|cr| rounded(cr, 9.0, 13.0, 14.0, 14.0, 4.0),
                    &|cr| rounded(cr, 9.0, 7.5, 3.2, 11.0, 1.6),
                    &|cr| rounded(cr, 12.6, 6.0, 3.2, 12.0, 1.6),
                    &|cr| rounded(cr, 16.2, 6.0, 3.2, 12.0, 1.6),
                    &|cr| rounded(cr, 19.8, 7.5, 3.2, 11.0, 1.6),
                ],
            );
        },
    },
    Cursor {
        name: "grabbing",
        hot: (16.0, 16.0),
        frames: 1,
        draw: |cr, _| {
            solid(
                cr,
                &[
                    &|cr| rounded(cr, 9.0, 14.0, 14.0, 12.0, 5.0),
                    &|cr| rounded(cr, 9.0, 11.0, 3.2, 6.0, 1.6),
                    &|cr| rounded(cr, 12.6, 10.0, 3.2, 7.0, 1.6),
                    &|cr| rounded(cr, 16.2, 10.0, 3.2, 7.0, 1.6),
                    &|cr| rounded(cr, 19.8, 11.0, 3.2, 6.0, 1.6),
                ],
            );
        },
    },
    Cursor {
        name: "help",
        hot: (6.0, 3.0),
        frames: 1,
        draw: |cr, _| {
            solid(cr, &[&arrow]);
            cr.select_font_face("sans-serif", FontSlant::Normal, FontWeight::Bold);
            cr.set_font_size(13.0);
            cr.move_to(19.5, 28.0);
            cr.text_path("?");
            cr.set_line_join(LineJoin::Round);
            cr.set_line_width(3.0);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.stroke_preserve();
            set_ink(cr);
            let _ = cr.fill();
        },
    },
    Cursor {
        name: "copy",
        hot: (6.0, 3.0),
        frames: 1,
        draw: |cr, _| {
            solid(cr, &[&arrow]);
            lines(
                cr,
                &|cr| {
                    cr.move_to(24.0, 19.5);
                    cr.line_to(24.0, 28.5);
                    cr.move_to(19.5, 24.0);
                    cr.line_to(28.5, 24.0);
                },
                4.4,
                2.4,
            );
        },
    },
    Cursor {
        name: "alias",
        hot: (6.0, 3.0),
        frames: 1,
        draw: |cr, _| {
            solid(cr, &[&arrow]);
            lines(
                cr,
                &|cr| {
                    cr.move_to(20.0, 28.0);
                    cr.line_to(27.0, 21.0);
                    cr.move_to(22.0, 21.0);
                    cr.line_to(27.0, 21.0);
                    cr.line_to(27.0, 26.0);
                },
                4.0,
                2.0,
            );
        },
    },
    Cursor {
        name: "context-menu",
        hot: (6.0, 3.0),
        frames: 1,
        draw: |cr, _| {
            solid(cr, &[&arrow]);
            lines(cr, &|cr| rounded(cr, 19.5, 21.0, 8.0, 6.5, 1.5), 4.0, 2.0);
        },
    },
    Cursor { name: "cell", hot: (16.0, 16.0), frames: 1, draw: |cr, _| solid(cr, &[&cell]) },
    Cursor { name: "zoom-in", hot: (13.5, 13.5), frames: 1, draw: |cr, _| lines(cr, &|cr| magnifier(cr, true), 4.4, 2.4) },
    Cursor { name: "zoom-out", hot: (13.5, 13.5), frames: 1, draw: |cr, _| lines(cr, &|cr| magnifier(cr, false), 4.4, 2.4) },
];

/// Every image of the cursor at every size, ready to write.
pub fn images(cursor: &Cursor) -> Vec<Image> {
    let mut images = Vec::new();
    for size in SIZES {
        for frame in 0..cursor.frames {
            let surface = render(cursor, size, frame);
            images.push(Image {
                size,
                width: size,
                height: size,
                hot: ((cursor.hot.0 * f64::from(size) / GRID).round() as u32, (cursor.hot.1 * f64::from(size) / GRID).round() as u32),
                delay: if cursor.frames > 1 { FRAME_MS } else { 0 },
                pixels: surface,
            });
        }
    }
    images
}

/// The cursor as premultiplied ARGB pixels, `size` square.
fn render(cursor: &Cursor, size: u32, frame: u32) -> Vec<u8> {
    let mut surface = ImageSurface::create(Format::ARgb32, size as i32, size as i32).expect("a cursor-sized surface");
    {
        let cr = Context::new(&surface).expect("a drawing context");
        cr.scale(f64::from(size) / GRID, f64::from(size) / GRID);
        (cursor.draw)(&cr, frame);
    }
    surface.flush();
    let data = surface.data().expect("pixels");
    data.to_vec()
}

/// Every cursor on a light strip and a dark one, with a red dot where each one clicks, to judge them by eye.
pub fn sheet(file: &Path) -> Result<(), String> {
    const CELL: i32 = 96;
    const SHOWN: f64 = 64.0;
    let columns = 8;
    let rows = CURSORS.len().div_ceil(columns) as i32;
    let surface = ImageSurface::create(Format::ARgb32, CELL * columns as i32, CELL * rows * 2).map_err(|error| error.to_string())?;
    let cr = Context::new(&surface).map_err(|error| error.to_string())?;
    for (strip, background) in [(0, (0.95, 0.94, 0.91)), (1, (0.12, 0.115, 0.10))] {
        cr.set_source_rgb(background.0, background.1, background.2);
        cr.rectangle(0.0, f64::from(strip * CELL * rows), f64::from(CELL * columns as i32), f64::from(CELL * rows));
        let _ = cr.fill();
        for (index, cursor) in CURSORS.iter().enumerate() {
            let (x, y) = ((index % columns) as i32 * CELL, strip * CELL * rows + (index / columns) as i32 * CELL);
            let picture = render(cursor, SHOWN as u32, 3);
            let tile = ImageSurface::create_for_data(picture, Format::ARgb32, SHOWN as i32, SHOWN as i32, SHOWN as i32 * 4).map_err(|error| error.to_string())?;
            let _ = cr.set_source_surface(&tile, f64::from(x) + (f64::from(CELL) - SHOWN) / 2.0, f64::from(y) + (f64::from(CELL) - SHOWN) / 2.0);
            let _ = cr.paint();
            cr.set_source_rgb(0.9, 0.1, 0.1);
            cr.arc(f64::from(x) + (f64::from(CELL) - SHOWN) / 2.0 + cursor.hot.0 * SHOWN / GRID, f64::from(y) + (f64::from(CELL) - SHOWN) / 2.0 + cursor.hot.1 * SHOWN / GRID, 1.5, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
    }
    drop(cr);
    let (width, height, stride) = (surface.width(), surface.height(), surface.stride() as usize);
    let mut surface = surface;
    surface.flush();
    let data = surface.data().map_err(|error| error.to_string())?.to_vec();
    let texture = gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::B8g8r8a8Premultiplied, &glib::Bytes::from_owned(data), stride);
    texture.save_to_png(file).map_err(|error| format!("{}: {error}", file.display()))
}

fn set_ink(cr: &Context) {
    cr.set_source_rgb(INK.0, INK.1, INK.2);
}

/// Closed shapes filled with ink and edged in white: the edge of every shape first, then the fills, so shapes that
/// touch (a finger and a palm) read as one.
fn solid(cr: &Context, shapes: &[&dyn Fn(&Context)]) {
    cr.set_line_join(LineJoin::Round);
    cr.set_line_width(2.4);
    for shape in shapes {
        cr.new_path();
        shape(cr);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.stroke();
    }
    for shape in shapes {
        cr.new_path();
        shape(cr);
        set_ink(cr);
        let _ = cr.fill();
    }
}

/// Open lines in ink, over a wider white line of the same path.
fn lines(cr: &Context, path: &dyn Fn(&Context), outer: f64, inner: f64) {
    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);
    cr.new_path();
    path(cr);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(outer);
    let _ = cr.stroke_preserve();
    set_ink(cr);
    cr.set_line_width(inner);
    let _ = cr.stroke();
}

/// Builds `shape` turned by `angle` about the middle of the grid.
fn rotated(cr: &Context, angle: f64, shape: &dyn Fn(&Context)) {
    cr.save().ok();
    cr.translate(GRID / 2.0, GRID / 2.0);
    cr.rotate(angle);
    cr.translate(-GRID / 2.0, -GRID / 2.0);
    shape(cr);
    cr.restore().ok();
}

fn polygon(cr: &Context, points: &[(f64, f64)]) {
    for (index, (x, y)) in points.iter().enumerate() {
        if index == 0 {
            cr.move_to(*x, *y);
        } else {
            cr.line_to(*x, *y);
        }
    }
    cr.close_path();
}

fn rounded(cr: &Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -PI / 2.0, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, PI / 2.0);
    cr.arc(x + radius, y + height - radius, radius, PI / 2.0, PI);
    cr.arc(x + radius, y + radius, radius, PI, 3.0 * PI / 2.0);
    cr.close_path();
}

fn arrow(cr: &Context) {
    polygon(cr, &[(6.0, 3.0), (6.0, 24.0), (11.0, 19.6), (14.8, 28.0), (18.4, 26.4), (14.7, 18.3), (21.0, 18.0)]);
}

fn double_arrow(cr: &Context) {
    polygon(cr, &[(4.0, 16.0), (10.0, 10.0), (10.0, 14.0), (22.0, 14.0), (22.0, 10.0), (28.0, 16.0), (22.0, 22.0), (22.0, 18.0), (10.0, 18.0), (10.0, 22.0)]);
}

fn four_arrows(cr: &Context) {
    polygon(
        cr,
        &[
            (16.0, 3.0),
            (21.0, 8.0),
            (18.0, 8.0),
            (18.0, 14.0),
            (24.0, 14.0),
            (24.0, 11.0),
            (29.0, 16.0),
            (24.0, 21.0),
            (24.0, 18.0),
            (18.0, 18.0),
            (18.0, 24.0),
            (21.0, 24.0),
            (16.0, 29.0),
            (11.0, 24.0),
            (14.0, 24.0),
            (14.0, 18.0),
            (8.0, 18.0),
            (8.0, 21.0),
            (3.0, 16.0),
            (8.0, 11.0),
            (8.0, 14.0),
            (14.0, 14.0),
            (14.0, 8.0),
            (11.0, 8.0),
        ],
    );
}

fn cell(cr: &Context) {
    polygon(cr, &[(13.6, 5.0), (18.4, 5.0), (18.4, 13.6), (27.0, 13.6), (27.0, 18.4), (18.4, 18.4), (18.4, 27.0), (13.6, 27.0), (13.6, 18.4), (5.0, 18.4), (5.0, 13.6), (13.6, 13.6)]);
}

fn text_beam(cr: &Context) {
    cr.move_to(12.0, 6.0);
    cr.line_to(20.0, 6.0);
    cr.move_to(16.0, 6.0);
    cr.line_to(16.0, 26.0);
    cr.move_to(12.0, 26.0);
    cr.line_to(20.0, 26.0);
}

fn crosshair(cr: &Context) {
    for (from, to) in [((16.0, 5.0), (16.0, 12.5)), ((16.0, 19.5), (16.0, 27.0)), ((5.0, 16.0), (12.5, 16.0)), ((19.5, 16.0), (27.0, 16.0))] {
        cr.move_to(from.0, from.1);
        cr.line_to(to.0, to.1);
    }
}

fn forbidden(cr: &Context) {
    cr.new_sub_path();
    cr.arc(16.0, 16.0, 9.0, 0.0, 2.0 * PI);
    cr.move_to(9.6, 9.6);
    cr.line_to(22.4, 22.4);
}

/// Three quarters of a circle, turned a twelfth more for each frame.
fn ring(cr: &Context, x: f64, y: f64, radius: f64, frame: u32) {
    let start = f64::from(frame) * 2.0 * PI / f64::from(TURN);
    cr.new_sub_path();
    cr.arc(x, y, radius, start, start + 1.5 * PI);
}

fn magnifier(cr: &Context, plus: bool) {
    cr.new_sub_path();
    cr.arc(13.5, 13.5, 7.5, 0.0, 2.0 * PI);
    cr.move_to(19.2, 19.2);
    cr.line_to(26.5, 26.5);
    cr.move_to(10.2, 13.5);
    cr.line_to(16.8, 13.5);
    if plus {
        cr.move_to(13.5, 10.2);
        cr.line_to(13.5, 16.8);
    }
}
