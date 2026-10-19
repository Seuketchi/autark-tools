//! A pixel buffer and the few shapes the splash draws: filled rounded rectangles, circles and a stroked outline, all with soft
//! edges. Pixels are `0x00RRGGBB`, the layout of the firmware framebuffer.

pub type Color = u32;

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

fn channels(c: Color) -> [f32; 3] {
    [((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32]
}

fn mix(under: Color, over: Color, alpha: f32) -> Color {
    let (a, b) = (channels(under), channels(over));
    let m = |i: usize| (a[i] + (b[i] - a[i]) * alpha).round().clamp(0.0, 255.0) as u32;
    (m(0) << 16) | (m(1) << 8) | m(2)
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Canvas {
        Canvas { width, height, pixels: vec![0; width * height] }
    }

    pub fn clear(&mut self, color: Color) {
        self.pixels.fill(color);
    }

    #[cfg(test)]
    pub fn at(&self, x: usize, y: usize) -> Color {
        self.pixels[y * self.width + x]
    }

    /// Lays `color` over one pixel with `alpha` coverage (0 to 1); outside the canvas is ignored.
    pub fn blend(&mut self, x: i32, y: i32, color: Color, alpha: f32) {
        if alpha <= 0.0 || x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let slot = &mut self.pixels[y as usize * self.width + x as usize];
        *slot = mix(*slot, color, alpha.min(1.0));
    }

    /// Fills where `coverage(x, y)` (given the pixel's centre) says, inside the box `x0..x1`, `y0..y1`.
    fn fill_with(&mut self, bounds: (f32, f32, f32, f32), color: Color, alpha: f32, coverage: impl Fn(f32, f32) -> f32) {
        let (x0, y0, x1, y1) = bounds;
        for y in (y0.floor() as i32 - 1)..=(y1.ceil() as i32 + 1) {
            for x in (x0.floor() as i32 - 1)..=(x1.ceil() as i32 + 1) {
                let covered = coverage(x as f32 + 0.5, y as f32 + 0.5);
                self.blend(x, y, color, covered * alpha);
            }
        }
    }

    /// A rectangle with corner radius `r` (a radius of half the height makes the ends round), at `alpha` opacity.
    #[allow(clippy::too_many_arguments)] // drawing primitives: where, how big, what colour
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color, alpha: f32) {
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let r = r.min(w / 2.0).min(h / 2.0);
        self.fill_with((x, y, x + w, y + h), color, alpha, |px, py| {
            let (qx, qy) = ((px - cx).abs() - (w / 2.0 - r), (py - cy).abs() - (h / 2.0 - r));
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let distance = outside + qx.max(qy).min(0.0) - r;
            (0.5 - distance).clamp(0.0, 1.0)
        });
    }

    pub fn circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, alpha: f32) {
        self.fill_with((cx - radius, cy - radius, cx + radius, cy + radius), color, alpha, |px, py| {
            let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius;
            (0.5 - distance).clamp(0.0, 1.0)
        });
    }

    /// A line of the given `width` along `points`, closed back to the first, with round joins.
    pub fn stroke_closed(&mut self, points: &[(f32, f32)], width: f32, color: Color, alpha: f32) {
        let half = width / 2.0;
        let xs = points.iter().map(|p| p.0);
        let ys = points.iter().map(|p| p.1);
        let bounds = (xs.clone().fold(f32::MAX, f32::min) - half, ys.clone().fold(f32::MAX, f32::min) - half, xs.fold(f32::MIN, f32::max) + half, ys.fold(f32::MIN, f32::max) + half);
        self.fill_with(bounds, color, alpha, |px, py| {
            let mut nearest = f32::MAX;
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                nearest = nearest.min(distance_to_segment((px, py), a, b));
            }
            (half - nearest + 0.5).clamp(0.0, 1.0)
        });
    }
}

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length_squared).clamp(0.0, 1.0) };
    ((p.0 - (a.0 + t * dx)).powi(2) + (p.1 - (a.1 + t * dy)).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filled_circle_is_solid_inside_and_untouched_outside() {
        let mut canvas = Canvas::new(40, 40);
        canvas.clear(0x000000);
        canvas.circle(20.0, 20.0, 10.0, 0xffffff, 1.0);
        assert_eq!(canvas.at(20, 20), 0xffffff);
        assert_eq!(canvas.at(2, 2), 0x000000);
        assert_eq!(canvas.at(20, 33), 0x000000);
    }

    #[test]
    fn the_edge_of_a_shape_is_between_the_two_colours() {
        let mut canvas = Canvas::new(40, 40);
        canvas.clear(0x000000);
        canvas.circle(20.0, 20.0, 10.3, 0xffffff, 1.0);
        let edge = canvas.at(30, 20) & 0xff;
        assert!(edge > 0 && edge < 0xff, "edge pixel was {edge:#x}");
    }

    #[test]
    fn opacity_scales_the_colour() {
        let mut canvas = Canvas::new(8, 8);
        canvas.clear(0x000000);
        canvas.round_rect(0.0, 0.0, 8.0, 8.0, 0.0, 0xffffff, 0.5);
        assert_eq!(canvas.at(4, 4) & 0xff, 128);
    }

    #[test]
    fn a_pill_has_round_ends() {
        let mut canvas = Canvas::new(100, 40);
        canvas.clear(0x000000);
        canvas.round_rect(10.0, 10.0, 80.0, 20.0, 10.0, 0xffffff, 1.0);
        assert_eq!(canvas.at(50, 20), 0xffffff);
        assert_eq!(canvas.at(11, 11), 0x000000, "the corner of the box is outside a pill");
        assert_eq!(canvas.at(12, 20), 0xffffff);
    }

    #[test]
    fn a_stroke_covers_the_line_and_not_the_middle_of_the_shape() {
        let mut canvas = Canvas::new(60, 60);
        canvas.clear(0x000000);
        canvas.stroke_closed(&[(10.0, 10.0), (50.0, 10.0), (50.0, 50.0), (10.0, 50.0)], 4.0, 0xffffff, 1.0);
        assert_eq!(canvas.at(30, 10), 0xffffff);
        assert_eq!(canvas.at(30, 30), 0x000000);
    }

    #[test]
    fn drawing_outside_the_canvas_is_ignored() {
        let mut canvas = Canvas::new(4, 4);
        canvas.circle(-20.0, -20.0, 5.0, 0xffffff, 1.0);
        canvas.blend(99, 99, 0xffffff, 1.0);
        assert!(canvas.pixels.iter().all(|&p| p == 0));
    }
}
