//! The splash as a [`Console`]: messages go under the name, a secret is typed into the box, and a thread repaints the screen
//! about 30 times a second (which also covers over anything the kernel prints onto the framebuffer meanwhile).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::termios::{LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr};
use zeroize::Zeroizing;

use super::canvas::Canvas;
use super::entry::{Entry, Key};
use super::fb::Framebuffer;
use super::scene::{Fonts, Mode, Scene, draw};
use crate::console::Console;
use crate::debug;

/// How long "loading" goes before the progress bar shows.
const BAR_AFTER: Duration = Duration::from_secs(3);
const FRAME: Duration = Duration::from_millis(33);

/// Where frames go: the framebuffer, or a recorder in the tests.
pub trait Screen: Send {
    fn size(&self) -> (usize, usize);
    fn present(&mut self, canvas: &Canvas);
}

impl Screen for Framebuffer {
    fn size(&self) -> (usize, usize) {
        (self.layout.width, self.layout.height)
    }

    fn present(&mut self, canvas: &Canvas) {
        Framebuffer::present(self, canvas);
    }
}

struct Shared {
    message: String,
    /// `None` while starting; the number of dots while asking or checking.
    asking: Option<usize>,
    checking: bool,
    loading_since: Option<Instant>,
    stop: bool,
}

pub struct SplashConsole {
    shared: Arc<Mutex<Shared>>,
    painter: Option<std::thread::JoinHandle<()>>,
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SplashConsole {
    /// An error says why there is no usable framebuffer or font: the caller falls back to the text prompt.
    pub fn start() -> Result<SplashConsole, String> {
        let screen = Framebuffer::open()?;
        let layout = screen.layout;
        debug::log(&format!("splash: the screen is {}x{}, stride {}, y offset {}, red/green/blue at bits {:?}", layout.width, layout.height, layout.stride, layout.y_offset, layout.shifts));
        Self::with_screen(screen).ok_or_else(|| "the splash's fonts didn't load".to_owned())
    }

    pub fn with_screen(mut screen: impl Screen + 'static) -> Option<SplashConsole> {
        let fonts = Fonts::load()?;
        // The kernel keeps the firmware's logo on screen until text is printed ("fbcon: Deferring console take-over"), and a frame
        // drawn before that is not shown. The first output wakes the console, but only a real character counts: the kernel ignores
        // output that is just blank cells (so ESC[2J alone did nothing). The dot is painted over by the first frame. Cursor off after.
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[2J\x1b[H.\x1b[?25l");
        let _ = out.flush();
        debug::log("splash: asked the console to take over the screen");
        let shared = Arc::new(Mutex::new(Shared { message: String::new(), asking: None, checking: false, loading_since: None, stop: false }));
        let painter = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("splash".into())
                .spawn(move || {
                    let (width, height) = screen.size();
                    let mut canvas = Canvas::new(width, height);
                    let started = Instant::now();
                    let mut first_frame = false;
                    loop {
                        let (scene, stop) = {
                            let state = lock(&shared);
                            let mode = match (state.asking, state.checking, state.loading_since) {
                                (_, _, Some(since)) => Mode::Loading { bar: since.elapsed() >= BAR_AFTER },
                                (Some(dots), true, None) => Mode::Checking { dots },
                                (Some(dots), false, None) => Mode::Asking { dots },
                                (None, _, None) => Mode::Starting,
                            };
                            (Scene { mode, message: state.message.clone() }, state.stop)
                        };
                        if stop {
                            return;
                        }
                        draw(&mut canvas, &fonts, &scene, started.elapsed().as_millis() as u64);
                        screen.present(&canvas);
                        if !first_frame {
                            first_frame = true;
                            debug::log("splash: the first frame is on the screen");
                        }
                        std::thread::sleep(FRAME);
                    }
                })
                .ok()?
        };
        Some(SplashConsole { shared, painter: Some(painter) })
    }

    /// The words above the box for what is being asked.
    fn ask_line(prompt: &str) -> String {
        if prompt.contains("PIN") {
            "Enter your disk PIN".to_owned()
        } else if prompt.contains("assphrase") {
            "Enter your recovery passphrase".to_owned()
        } else {
            prompt.trim().trim_end_matches(':').to_owned()
        }
    }

    /// The system is up; until systemd draws something else, show that it is loading.
    pub fn unlocked(&mut self) {
        let mut state = lock(&self.shared);
        state.asking = None;
        state.checking = false;
        state.message.clear();
        state.loading_since = Some(Instant::now());
    }

    /// Puts the box up for `prompt`, reads an entry with `read`, then shows the check running.
    fn ask(&mut self, prompt: &str, read: impl FnOnce(&mut Self) -> Option<Entry>) -> Option<Zeroizing<Vec<u8>>> {
        {
            let mut state = lock(&self.shared);
            // The last answer's complaint ("That PIN isn't right.") stays until the next try is answered; otherwise say what is asked.
            if !state.message.contains("isn't right") {
                state.message = Self::ask_line(prompt);
            }
            state.asking = Some(0);
            state.checking = false;
        }
        debug::log(&format!("asking: {}", lock(&self.shared).message));
        let entry = read(self)?;
        debug::log("an answer was entered");
        let mut state = lock(&self.shared);
        state.checking = true;
        state.message.clear();
        Some(entry.into_secret())
    }

    /// Reads a line from the terminal with echo off.
    fn read_entry(&mut self) -> Option<Entry> {
        let input = rustix::stdio::stdin();
        let saved = tcgetattr(input).ok();
        if let Some(saved) = &saved {
            let mut raw = saved.clone();
            raw.local_modes.remove(LocalModes::ECHO | LocalModes::ICANON);
            raw.special_codes[SpecialCodeIndex::VMIN] = 1;
            raw.special_codes[SpecialCodeIndex::VTIME] = 0;
            let _ = tcsetattr(input, OptionalActions::Flush, &raw);
        }
        let entry = self.read_from(&mut std::io::stdin().lock());
        if let Some(saved) = &saved {
            let _ = tcsetattr(input, OptionalActions::Flush, saved);
        }
        entry
    }

    /// Reads keys until Enter, showing a dot for each; `None` when the input ends first.
    fn read_from(&mut self, input: &mut impl Read) -> Option<Entry> {
        let mut entry = Entry::default();
        let mut first_key = true;
        loop {
            let mut byte = [0u8; 1];
            match input.read(&mut byte) {
                Ok(1) => {
                    if first_key {
                        first_key = false;
                        debug::log("a key reached the splash");
                    }
                    match entry.feed(byte[0]) {
                        Key::Done => return Some(entry),
                        Key::Edited => lock(&self.shared).asking = Some(entry.characters()),
                    }
                }
                _ => return None,
            }
        }
    }
}

impl Console for SplashConsole {
    fn say(&mut self, text: &str) {
        let mut state = lock(&self.shared);
        state.message = text.to_owned();
        // A message after a wrong answer means the check is over and a new one is coming.
        state.checking = false;
    }

    fn secret(&mut self, prompt: &str) -> Option<Zeroizing<Vec<u8>>> {
        self.ask(prompt, |this| this.read_entry())
    }

    fn wait_for_enter(&mut self, text: &str) {
        self.say(text);
        let mut byte = [0u8; 1];
        let _ = std::io::stdin().lock().read(&mut byte);
    }

    fn unlocked(&mut self) {
        SplashConsole::unlocked(self);
    }
}

impl Drop for SplashConsole {
    fn drop(&mut self) {
        lock(&self.shared).stop = true;
        if let Some(painter) = self.painter.take() {
            let _ = painter.join();
        }
        // Hand the screen back to the text console: clear it and bring the cursor back.
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[2J\x1b[H\x1b[?25h");
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::splash::scene::{Mode, Scene};

    /// Keeps the last frame it was given.
    struct Recorder(Arc<Mutex<Option<Canvas>>>);

    impl Screen for Recorder {
        fn size(&self) -> (usize, usize) {
            (640, 400)
        }

        fn present(&mut self, canvas: &Canvas) {
            *self.0.lock().unwrap() = Some(Canvas { width: canvas.width, height: canvas.height, pixels: canvas.pixels.clone() });
        }
    }

    fn start() -> (SplashConsole, Arc<Mutex<Option<Canvas>>>) {
        let frames = Arc::new(Mutex::new(None));
        (SplashConsole::with_screen(Recorder(Arc::clone(&frames))).unwrap(), frames)
    }

    fn expected(mode: Mode, message: &str) -> Vec<u32> {
        let mut canvas = Canvas::new(640, 400);
        draw(&mut canvas, &Fonts::load().unwrap(), &Scene { mode, message: message.to_owned() }, 0);
        canvas.pixels
    }

    fn settle() {
        std::thread::sleep(Duration::from_millis(150));
    }

    fn shown(frames: &Arc<Mutex<Option<Canvas>>>) -> Vec<u32> {
        frames.lock().unwrap().as_ref().expect("nothing was drawn").pixels.clone()
    }

    #[test]
    fn a_pin_is_read_through_the_box_and_comes_back_whole() {
        let (mut console, frames) = start();
        let got = console.ask("TPM PIN: ", |this| this.read_from(&mut Cursor::new(b"2468\r".to_vec())));
        assert_eq!(&got.unwrap()[..], b"2468");
        settle();
        // Typed and handed over: the box dims (the answer is being checked) with the four dots still in it. The pulse depends on the
        // time, so compare only the part below the mark: the box and what is under it.
        let rows = |pixels: &[u32]| pixels[250 * 640..].to_vec();
        assert_eq!(rows(&shown(&frames)), rows(&expected(Mode::Checking { dots: 4 }, "")));
    }

    #[test]
    fn the_box_shows_the_question_and_a_dot_per_key_while_typing() {
        let (mut console, frames) = start();
        let mut partial = Cursor::new(b"135".to_vec());
        let got = console.ask("TPM PIN: ", |this| this.read_from(&mut partial));
        assert!(got.is_none(), "the input ended before Enter, so there is no secret");
        settle();
        assert_eq!(shown(&frames), expected(Mode::Asking { dots: 3 }, "Enter your disk PIN"));
    }

    #[test]
    fn a_wrong_answer_keeps_its_message_over_the_next_ask_and_the_passphrase_has_its_own_question() {
        let (mut console, frames) = start();
        console.say("That PIN isn't right.");
        let _ = console.ask("TPM PIN: ", |_| None);
        settle();
        assert_eq!(shown(&frames), expected(Mode::Asking { dots: 0 }, "That PIN isn't right."));
        console.say("Use the passphrase or the recovery key instead.");
        let _ = console.ask("Passphrase or recovery key: ", |_| None);
        settle();
        assert_eq!(shown(&frames), expected(Mode::Asking { dots: 0 }, "Enter your recovery passphrase"));
    }

    #[test]
    fn unlocked_clears_the_box_and_starts_loading() {
        let (mut console, frames) = start();
        let _ = console.ask("TPM PIN: ", |this| this.read_from(&mut Cursor::new(b"1\r".to_vec())));
        console.unlocked();
        settle();
        // Under three seconds there is no bar: nothing below the name.
        let below = |pixels: &[u32]| pixels[250 * 640..].iter().filter(|&&p| p != crate::splash::scene::SKY).count();
        assert_eq!(below(&shown(&frames)), 0);
    }

    #[test]
    fn dropping_the_console_stops_the_painter() {
        let (console, frames) = start();
        settle();
        drop(console);
        let before = shown(&frames);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(shown(&frames), before);
    }
}
