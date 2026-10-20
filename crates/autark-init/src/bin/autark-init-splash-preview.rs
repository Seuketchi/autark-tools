//! Draws every state of the boot splash into PPM pictures, to look at without booting (`autark-init-splash-preview <out-dir> [WxH]`).

// The splash module reaches for `crate::console::Console`, so the real one is included too.
#[path = "../console.rs"]
#[allow(dead_code)]
mod console;

#[path = "../debug.rs"]
#[allow(dead_code)]
mod debug;

#[path = "../splash/mod.rs"]
#[allow(dead_code)]
mod splash;

use splash::canvas::Canvas;
use splash::scene::{Fonts, Mode, Scene, draw};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| {
        eprintln!("usage: autark-init-splash-preview <out-dir> [WxH]");
        std::process::exit(2)
    });
    let (w, h) = args.next().and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))).unwrap_or((1920, 1080));
    let fonts = Fonts::load().expect("fonts");
    let states = [
        ("starting", Mode::Starting, "", 0),
        ("pin", Mode::Asking { dots: 4 }, "Enter your disk PIN", 0),
        ("checking", Mode::Checking { dots: 4 }, "", 1200),
        ("wrong", Mode::Asking { dots: 0 }, "That PIN isn’t right.", 0),
        ("loading", Mode::Loading { bar: true }, "", 700),
    ];
    for (name, mode, message, ms) in states {
        let mut canvas = Canvas::new(w, h);
        draw(&mut canvas, &fonts, &Scene { mode, message: message.to_owned() }, ms);
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        for p in &canvas.pixels {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        let path = format!("{dir}/{name}.ppm");
        std::fs::write(&path, out).unwrap_or_else(|e| panic!("{path}: {e}"));
        println!("{path}");
    }
}
