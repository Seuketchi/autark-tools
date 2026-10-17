//! The sidebar's glyphs, drawn from the design's SVG path data with cairo (no SVG loader on Autark). Only what
//! the paths use: moves, lines, arcs and closes, absolute and relative, on a 24-unit grid.

use gtk::cairo::Context;

/// Strokes `path` scaled from 24 units to `size` pixels.
pub fn stroke(cr: &Context, path: &str, size: f64) {
    cr.save().ok();
    cr.scale(size / 24.0, size / 24.0);
    trace(cr, path);
    cr.restore().ok();
    cr.set_line_width(2.0 * size / 24.0);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    cr.stroke().ok();
}

fn numbers(text: &str) -> Vec<f64> {
    // "0 0 1 20 0", "-1.8-4.7" and ".01" all come apart here.
    let mut values = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, values: &mut Vec<f64>| {
        if let Ok(value) = current.parse() {
            values.push(value);
        }
        current.clear();
    };
    for c in text.chars() {
        match c {
            '-' if !current.is_empty() && !current.ends_with('e') => {
                flush(&mut current, &mut values);
                current.push(c);
            }
            '.' if current.contains('.') => {
                flush(&mut current, &mut values);
                current.push(c);
            }
            '0'..='9' | '.' | '-' | 'e' => current.push(c),
            _ => flush(&mut current, &mut values),
        }
    }
    flush(&mut current, &mut values);
    values
}

fn trace(cr: &Context, path: &str) {
    let mut commands = Vec::new();
    let mut start = 0;
    for (index, c) in path.char_indices() {
        if c.is_ascii_alphabetic() && c != 'e' {
            if index > 0 {
                commands.push(&path[start..index]);
            }
            start = index;
        }
    }
    commands.push(&path[start..]);

    let (mut x, mut y) = (0.0, 0.0);
    let (mut first_x, mut first_y) = (0.0, 0.0);
    for command in commands {
        let Some(letter) = command.chars().next() else { continue };
        let values = numbers(&command[1..]);
        let relative = letter.is_ascii_lowercase();
        let (base_x, base_y) = if relative { (x, y) } else { (0.0, 0.0) };
        match letter.to_ascii_uppercase() {
            'M' | 'L' => {
                for (index, pair) in values.as_chunks::<2>().0.iter().enumerate() {
                    let (px, py) = if relative { (x + pair[0], y + pair[1]) } else { (pair[0], pair[1]) };
                    if letter.eq_ignore_ascii_case(&'M') && index == 0 {
                        cr.move_to(px, py);
                        (first_x, first_y) = (px, py);
                    } else {
                        cr.line_to(px, py);
                    }
                    (x, y) = (px, py);
                }
            }
            'H' => {
                for value in values {
                    x = base_x + value;
                    cr.line_to(x, y);
                }
            }
            'V' => {
                for value in values {
                    y = base_y + value;
                    cr.line_to(x, y);
                }
            }
            'C' => {
                for curve in values.as_chunks::<6>().0 {
                    let point = |index: usize| if relative { (x + curve[index], y + curve[index + 1]) } else { (curve[index], curve[index + 1]) };
                    let ((x1, y1), (x2, y2), (ex, ey)) = (point(0), point(2), point(4));
                    cr.curve_to(x1, y1, x2, y2, ex, ey);
                    (x, y) = (ex, ey);
                }
            }
            'A' => {
                for arc in values.as_chunks::<7>().0 {
                    let (ex, ey) = if relative { (x + arc[5], y + arc[6]) } else { (arc[5], arc[6]) };
                    arc_to(cr, (x, y), arc[0], arc[1], arc[3] != 0.0, arc[4] != 0.0, (ex, ey));
                    (x, y) = (ex, ey);
                }
            }
            'Z' => {
                cr.close_path();
                (x, y) = (first_x, first_y);
            }
            _ => {}
        }
    }
}

/// An SVG elliptical arc (no rotation, which the glyphs don't use) as cairo arcs.
fn arc_to(cr: &Context, from: (f64, f64), rx: f64, ry: f64, large: bool, sweep: bool, to: (f64, f64)) {
    let (mut rx, ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        cr.line_to(to.0, to.1);
        return;
    }
    // Work on a circle, then stretch back to the ellipse.
    let ratio = ry / rx;
    let (x1, y1) = (from.0, from.1 / ratio);
    let (x2, y2) = (to.0, to.1 / ratio);
    let (dx, dy) = ((x2 - x1) / 2.0, (y2 - y1) / 2.0);
    let half = (dx * dx + dy * dy).sqrt();
    if half == 0.0 {
        return;
    }
    rx = rx.max(half);
    let offset = (rx * rx - half * half).max(0.0).sqrt() * if large == sweep { -1.0 } else { 1.0 };
    let (mx, my) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
    let (cx, cy) = (mx - offset * dy / half, my + offset * dx / half);
    let a1 = (y1 - cy).atan2(x1 - cx);
    let a2 = (y2 - cy).atan2(x2 - cx);
    cr.save().ok();
    cr.scale(1.0, ratio);
    if sweep {
        cr.arc(cx, cy, rx, a1, a2);
    } else {
        cr.arc_negative(cx, cy, rx, a1, a2);
    }
    cr.restore().ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_split_like_svg() {
        assert_eq!(numbers("-1.8-4.7"), vec![-1.8, -4.7]);
        assert_eq!(numbers(" 15 15 0 0 1 20 0"), vec![15.0, 15.0, 0.0, 0.0, 1.0, 20.0, 0.0]);
        assert_eq!(numbers(".01"), vec![0.01]);
        assert_eq!(numbers("3.5.5"), vec![3.5, 0.5]);
    }
}
