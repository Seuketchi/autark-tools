//! VP8 encoding through libvpx (see `csrc/vpx_shim.c`): planar YUV frames in, compressed packets out.

use std::ffi::c_void;

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

type Packet = extern "C" fn(user: *mut c_void, data: *const u8, len: usize, key: i32, pts: i64);

unsafe extern "C" {
    fn rec_enc_new(width: u32, height: u32, fps: u32, kbps: u32) -> *mut Raw;
    #[allow(clippy::too_many_arguments)]
    fn rec_enc_frame(
        enc: *mut Raw,
        y: *const u8,
        y_stride: i32,
        u: *const u8,
        u_stride: i32,
        v: *const u8,
        v_stride: i32,
        width: u32,
        height: u32,
        pts: i64,
        force_key: i32,
        write: Packet,
        user: *mut c_void,
    ) -> i32;
    fn rec_enc_flush(enc: *mut Raw, write: Packet, user: *mut c_void) -> i32;
    fn rec_enc_free(enc: *mut Raw);
}

/// One picture as three planes: full-size luma, and chroma at half the width and height.
pub struct Yuv {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Yuv {
    /// `width` and `height` are even.
    pub fn new(width: u32, height: u32) -> Self {
        let (w, h) = (width as usize, height as usize);
        Self { width, height, y: vec![0; w * h], u: vec![128; w * h / 4], v: vec![128; w * h / 4] }
    }
}

type Sink<'a> = &'a mut dyn FnMut(&[u8], bool, i64);

extern "C" fn trampoline(user: *mut c_void, data: *const u8, len: usize, key: i32, pts: i64) {
    // SAFETY: `user` is the `&mut Sink` that `encode` or `flush` passed down and is still alive during the call;
    // `data` is `len` bytes owned by libvpx until the call returns.
    let (sink, data) = unsafe { (&mut *user.cast::<Sink>(), std::slice::from_raw_parts(data, len)) };
    sink(data, key != 0, pts);
}

pub struct Encoder {
    raw: *mut Raw,
}

impl Encoder {
    pub fn new(width: u32, height: u32, fps: u32, kbps: u32) -> Result<Self, String> {
        // SAFETY: plain values in, a pointer or null out.
        let raw = unsafe { rec_enc_new(width, height, fps, kbps) };
        if raw.is_null() { Err("libvpx couldn't start a VP8 encoder".to_owned()) } else { Ok(Self { raw }) }
    }

    /// Encodes one picture at `pts` milliseconds, handing each packet it finishes to `sink` with whether it's a key frame.
    pub fn encode(&mut self, frame: &Yuv, pts: i64, key: bool, mut sink: impl FnMut(&[u8], bool, i64)) -> Result<(), String> {
        let mut sink: Sink = &mut sink;
        // SAFETY: the planes are as large as `width` and `height` say, `Yuv::new` makes them so, and `sink` outlives the call.
        let status = unsafe {
            rec_enc_frame(
                self.raw,
                frame.y.as_ptr(),
                frame.width as i32,
                frame.u.as_ptr(),
                (frame.width / 2) as i32,
                frame.v.as_ptr(),
                (frame.width / 2) as i32,
                frame.width,
                frame.height,
                pts,
                i32::from(key),
                trampoline,
                (&raw mut sink).cast(),
            )
        };
        if status == 0 { Ok(()) } else { Err("libvpx failed to encode a frame".to_owned()) }
    }

    /// Hands over what's still held back.
    pub fn flush(&mut self, mut sink: impl FnMut(&[u8], bool, i64)) -> Result<(), String> {
        let mut sink: Sink = &mut sink;
        // SAFETY: as in `encode`.
        let status = unsafe { rec_enc_flush(self.raw, trampoline, (&raw mut sink).cast()) };
        if status == 0 { Ok(()) } else { Err("libvpx failed to finish the video".to_owned()) }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `rec_enc_new` and is freed once.
        unsafe { rec_enc_free(self.raw) }
    }
}
