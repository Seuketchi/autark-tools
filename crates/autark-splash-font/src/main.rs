//! Renders the glyphs the boot splash draws (`autark-init`'s `splash` module) into `.bin` files it includes at build time, so
//! the initramfs carries about 40 KB of pre-drawn letters instead of a font and a font library.
//!
//! ```text
//! autark-splash-font <out-dir>      writes geist-regular.bin and geist-semibold.bin
//! ```
//!
//! File format, little endian: `"ASF1"`, base pixel size (u8), ascent (u8), descent (u8), glyph count (u16); then per glyph
//! code point (u16), advance in 1/16 px (u16), left bearing (i8), top above the baseline (i8), width (u8), height (u8),
//! offset into the alpha data (u32); then every glyph's width × height alpha bytes.

use std::path::Path;

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface};

/// The size the glyphs are drawn at; the splash scales them down (the largest it shows is about this size).
const BASE: f64 = 48.0;

fn glyph_set() -> Vec<char> {
    (0x20u8..=0x7E).map(char::from).chain(['…', '’']).collect()
}

fn main() {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: autark-splash-font <out-dir>");
        std::process::exit(2);
    };
    for (file, family) in [("geist-regular.bin", "Geist"), ("geist-semibold.bin", "Geist SemiBold")] {
        let bytes = render(family);
        let path = Path::new(&out).join(file);
        std::fs::write(&path, &bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        println!("{}: {} bytes", path.display(), bytes.len());
    }
}

fn render(family: &str) -> Vec<u8> {
    let scratch = ImageSurface::create(Format::A8, 1, 1).expect("surface");
    let measure = Context::new(&scratch).expect("context");
    measure.select_font_face(family, FontSlant::Normal, FontWeight::Normal);
    measure.set_font_size(BASE);
    let font = measure.font_extents().expect("font extents");

    let mut table = Vec::new();
    let mut alpha: Vec<u8> = Vec::new();
    let glyphs = glyph_set();
    for ch in &glyphs {
        let text = ch.to_string();
        let extents = measure.text_extents(&text).expect("text extents");
        let (left, top) = (extents.x_bearing().floor(), (-extents.y_bearing()).ceil());
        let width = (extents.x_bearing() + extents.width()).ceil() - left;
        let height = (extents.y_bearing() + extents.height()).ceil() + top;
        let (w, h) = (width.max(0.0) as i32, height.max(0.0) as i32);
        let offset = alpha.len() as u32;
        if w > 0 && h > 0 {
            let mut surface = ImageSurface::create(Format::A8, w, h).expect("surface");
            {
                let ctx = Context::new(&surface).expect("context");
                ctx.select_font_face(family, FontSlant::Normal, FontWeight::Normal);
                ctx.set_font_size(BASE);
                ctx.set_source_rgba(0.0, 0.0, 0.0, 1.0);
                ctx.move_to(-left, top);
                ctx.show_text(&text).expect("show");
            }
            surface.flush();
            let stride = surface.stride() as usize;
            let data = surface.data().expect("data");
            for y in 0..h as usize {
                alpha.extend_from_slice(&data[y * stride..y * stride + w as usize]);
            }
        }
        table.extend_from_slice(&(*ch as u16).to_le_bytes());
        table.extend_from_slice(&((extents.x_advance() * 16.0).round() as u16).to_le_bytes());
        table.push(left as i8 as u8);
        table.push(top as i8 as u8);
        table.push(w as u8);
        table.push(h as u8);
        table.extend_from_slice(&offset.to_le_bytes());
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"ASF1");
    out.push(BASE as u8);
    out.push(font.ascent().ceil() as u8);
    out.push(font.descent().ceil() as u8);
    out.extend_from_slice(&(glyphs.len() as u16).to_le_bytes());
    out.extend_from_slice(&table);
    out.extend_from_slice(&alpha);
    out
}
