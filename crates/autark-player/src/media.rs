//! Playback through GStreamer (see `csrc/play_shim.c`): open a URI, play, pause, seek, and take the newest picture
//! as packed BGRx. Used from one thread, the one that draws.

use std::ffi::{CString, c_char, c_int, c_longlong};

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn player_new(uri: *const c_char, err: *mut c_char, errlen: usize) -> *mut Raw;
    fn player_play(player: *mut Raw, playing: c_int);
    fn player_poll(player: *mut Raw, err: *mut c_char, errlen: usize) -> c_int;
    fn player_position_ms(player: *mut Raw) -> c_longlong;
    fn player_duration_ms(player: *mut Raw) -> c_longlong;
    fn player_seek_ms(player: *mut Raw, ms: c_longlong);
    fn player_volume(player: *mut Raw, volume: f64, mute: c_int);
    fn player_frame(player: *mut Raw, dst: *mut u8, cap: usize, w: *mut c_int, h: *mut c_int) -> c_int;
    fn player_free(player: *mut Raw);
}

/// What the pipeline has to say.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// It played to the end.
    Ended,
    Failed(String),
    /// Prerolled: the duration is known and the first picture is there.
    Ready,
}

pub struct Player {
    raw: *mut Raw,
    pixels: Vec<u8>,
    size: (u32, u32),
}

/// Text a C function wrote into a buffer.
fn text(buffer: &[u8]) -> String {
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

impl Player {
    /// Opens `uri` paused, on its first picture.
    pub fn open(uri: &str) -> Result<Self, String> {
        let uri = CString::new(uri).map_err(|_| "the address has a null byte in it".to_owned())?;
        let mut error = [0u8; 256];
        // SAFETY: `uri` is a C string and `error` is as long as the length says.
        let raw = unsafe { player_new(uri.as_ptr(), error.as_mut_ptr().cast(), error.len()) };
        if raw.is_null() { Err(text(&error)) } else { Ok(Self { raw, pixels: Vec::new(), size: (0, 0) }) }
    }

    pub fn play(&mut self, playing: bool) {
        // SAFETY: `raw` is live until `drop`.
        unsafe { player_play(self.raw, c_int::from(playing)) }
    }

    /// The next thing the pipeline said, if it said anything.
    pub fn poll(&mut self) -> Option<Event> {
        let mut error = [0u8; 512];
        // SAFETY: as in `open`.
        match unsafe { player_poll(self.raw, error.as_mut_ptr().cast(), error.len()) } {
            1 => Some(Event::Ended),
            2 => Some(Event::Failed(text(&error))),
            3 => Some(Event::Ready),
            _ => None,
        }
    }

    pub fn position_ms(&self) -> Option<i64> {
        // SAFETY: `raw` is live until `drop`.
        Some(unsafe { player_position_ms(self.raw) }).filter(|ms| *ms >= 0)
    }

    pub fn duration_ms(&self) -> Option<i64> {
        // SAFETY: as above.
        Some(unsafe { player_duration_ms(self.raw) }).filter(|ms| *ms > 0)
    }

    pub fn seek_ms(&mut self, ms: i64) {
        // SAFETY: as above.
        unsafe { player_seek_ms(self.raw, ms) }
    }

    /// `volume` is 0 to 1.
    pub fn set_volume(&mut self, volume: f64, mute: bool) {
        // SAFETY: as above.
        unsafe { player_volume(self.raw, volume, c_int::from(mute)) }
    }

    /// Takes the newest picture if there's a new one: its width and height, with the pixels in `pixels`.
    pub fn frame(&mut self) -> Option<(u32, u32)> {
        loop {
            let (mut w, mut h): (c_int, c_int) = (0, 0);
            // SAFETY: `pixels` is as long as `capacity` says; the C side writes no more.
            let status = unsafe { player_frame(self.raw, self.pixels.as_mut_ptr(), self.pixels.len(), &mut w, &mut h) };
            match status {
                1 => {
                    self.size = (w as u32, h as u32);
                    return Some(self.size);
                }
                // Too small for this size: the picture was dropped, so make room and look again for the next.
                -1 => self.pixels.resize(w as usize * h as usize * 4, 0),
                _ => return None,
            }
        }
    }

    /// The last picture `frame` took, packed BGRx.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels[..self.size.0 as usize * self.size.1 as usize * 4]
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `player_new` and is freed once.
        unsafe { player_free(self.raw) }
    }
}
