//! Letters, from the alpha masks `autark-splash-font` drew from Geist (`assets/*.bin`, format described there). The masks are
//! drawn at one size and scaled down by averaging the source pixels each destination pixel covers.

use super::canvas::{Canvas, Color};

pub struct Font {
    size: f32,
    glyphs: Vec<Glyph>,
    alpha: &'static [u8],
}

struct Glyph {
    ch: u16,
    advance: f32,
    left: i32,
    top: i32,
    w: usize,
    h: usize,
    offset: usize,
}

pub static REGULAR: &[u8] = include_bytes!("../../assets/geist-regular.bin");
pub static SEMIBOLD: &[u8] = include_bytes!("../../assets/geist-semibold.bin");

impl Font {
    pub fn parse(bytes: &'static [u8]) -> Option<Font> {
        if bytes.get(..4)? != b"ASF1" {
            return None;
        }
        let size = *bytes.get(4)? as f32;
        let count = u16::from_le_bytes([*bytes.get(7)?, *bytes.get(8)?]) as usize;
        let table = bytes.get(9..9 + count * 12)?;
        let glyphs = table
            .chunks_exact(12)
            .map(|g| Glyph {
                ch: u16::from_le_bytes([g[0], g[1]]),
                advance: u16::from_le_bytes([g[2], g[3]]) as f32 / 16.0,
                left: g[4] as i8 as i32,
                top: g[5] as i8 as i32,
                w: g[6] as usize,
                h: g[7] as usize,
                offset: u32::from_le_bytes([g[8], g[9], g[10], g[11]]) as usize,
            })
            .collect();
        Some(Font { size, glyphs, alpha: bytes.get(9 + count * 12..)? })
    }

    fn glyph(&self, ch: char) -> Option<&Glyph> {
        let code = u16::try_from(ch as u32).ok()?;
        self.glyphs.iter().find(|g| g.ch == code)
    }

    /// How wide `text` is when drawn at `px` pixels tall.
    pub fn width(&self, text: &str, px: f32) -> f32 {
        let scale = px / self.size;
        text.chars().filter_map(|c| self.glyph(c)).map(|g| g.advance * scale).sum()
    }

    /// Draws `text` with its left edge at `x` and its baseline at `y`.
    #[allow(clippy::too_many_arguments)] // drawing primitives: where, how big, what colour
    pub fn draw(&self, canvas: &mut Canvas, text: &str, x: f32, y: f32, px: f32, color: Color, opacity: f32) {
        let scale = px / self.size;
        let mut pen = x;
        for ch in text.chars() {
            let Some(g) = self.glyph(ch) else { continue };
            if g.w > 0 && g.h > 0 {
                self.draw_glyph(canvas, g, pen + g.left as f32 * scale, y - g.top as f32 * scale, scale, color, opacity);
            }
            pen += g.advance * scale;
        }
    }

    #[allow(clippy::too_many_arguments)] // drawing primitives: where, how big, what colour
    fn draw_glyph(&self, canvas: &mut Canvas, g: &Glyph, x: f32, y: f32, scale: f32, color: Color, opacity: f32) {
        let (out_w, out_h) = ((g.w as f32 * scale).ceil() as i32 + 1, (g.h as f32 * scale).ceil() as i32 + 1);
        let (x0, y0) = (x.floor() as i32, y.floor() as i32);
        for oy in 0..out_h {
            for ox in 0..out_w {
                // The source rectangle this destination pixel covers.
                let sx0 = ((x0 + ox) as f32 - x) / scale;
                let sy0 = ((y0 + oy) as f32 - y) / scale;
                let coverage = self.average(g, sx0, sy0, sx0 + 1.0 / scale, sy0 + 1.0 / scale);
                canvas.blend(x0 + ox, y0 + oy, color, coverage * opacity);
            }
        }
    }

    /// The mean alpha of the source pixels in the rectangle (sub-pixel edges weighted by how much they are covered).
    fn average(&self, g: &Glyph, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
        let (mut sum, mut area) = (0.0, 0.0);
        let mut sy = y0.floor();
        while sy < y1 {
            let weight_y = (sy + 1.0).min(y1) - sy.max(y0);
            let mut sx = x0.floor();
            while sx < x1 {
                let weight = weight_y * ((sx + 1.0).min(x1) - sx.max(x0));
                area += weight;
                if sx >= 0.0 && sy >= 0.0 && (sx as usize) < g.w && (sy as usize) < g.h {
                    sum += weight * self.alpha[g.offset + sy as usize * g.w + sx as usize] as f32 / 255.0;
                }
                sx += 1.0;
            }
            sy += 1.0;
        }
        if area > 0.0 { sum / area } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_shipped_fonts_parse_and_have_the_letters_the_splash_says() {
        for bytes in [REGULAR, SEMIBOLD] {
            let font = Font::parse(bytes).expect("font file");
            for ch in "Autark Enter your disk PIN. Wrong PIN. That isn’t right… 0123456789".chars() {
                assert!(font.glyph(ch).is_some(), "no glyph for {ch:?}");
            }
        }
    }

    #[test]
    fn text_has_a_width_that_scales_with_its_size() {
        let font = Font::parse(REGULAR).unwrap();
        let (small, large) = (font.width("Autark", 20.0), font.width("Autark", 40.0));
        assert!(small > 20.0 && (large / small - 2.0).abs() < 0.01, "{small} {large}");
    }

    #[test]
    fn drawing_text_puts_ink_on_the_canvas_and_only_near_the_text() {
        let font = Font::parse(SEMIBOLD).unwrap();
        let mut canvas = Canvas::new(200, 60);
        canvas.clear(0x000000);
        font.draw(&mut canvas, "Autark", 10.0, 40.0, 30.0, 0xffffff, 1.0);
        let lit = |y0: usize, y1: usize| (y0..y1).flat_map(|y| (0..200).map(move |x| (x, y))).filter(|&(x, y)| canvas.at(x, y) != 0).count();
        assert!(lit(10, 45) > 150, "the text left almost no ink");
        assert_eq!(lit(0, 8), 0);
        assert_eq!(lit(50, 60), 0, "no descenders in Autark: nothing below the baseline");
    }

    #[test]
    fn an_unknown_character_is_skipped_without_a_panic() {
        let font = Font::parse(REGULAR).unwrap();
        let mut canvas = Canvas::new(80, 30);
        font.draw(&mut canvas, "a\u{1F600}b", 2.0, 20.0, 16.0, 0xffffff, 1.0);
        assert!(font.width("\u{1F600}", 16.0) == 0.0);
    }

    #[test]
    fn a_damaged_file_is_refused() {
        assert!(Font::parse(b"nope").is_none());
        assert!(Font::parse(&REGULAR[..20]).is_none());
    }
}
