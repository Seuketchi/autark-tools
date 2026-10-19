//! Opus encoding through libopus: 20 ms of 48 kHz stereo in, one packet out.

use std::ffi::c_int;

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn opus_encoder_create(rate: i32, channels: c_int, application: c_int, error: *mut c_int) -> *mut Raw;
    fn opus_encode_float(encoder: *mut Raw, pcm: *const f32, frame_size: c_int, data: *mut u8, max: i32) -> i32;
    fn opus_encoder_ctl(encoder: *mut Raw, request: c_int, ...) -> c_int;
    fn opus_encoder_destroy(encoder: *mut Raw);
}

const APPLICATION_AUDIO: c_int = 2049;
const SET_BITRATE: c_int = 4002;
const GET_LOOKAHEAD: c_int = 4027;

/// Samples in one 20 ms frame, for each channel.
pub const FRAME: usize = 960;
pub const CHANNELS: usize = 2;
pub const RATE: u32 = 48_000;

pub struct Encoder {
    raw: *mut Raw,
    /// Samples the decoder drops from the start, which the file's header says.
    pub pre_skip: u16,
}

impl Encoder {
    pub fn new(bitrate: i32) -> Result<Self, String> {
        let mut error = 0;
        // SAFETY: plain values in, a pointer or null out.
        let raw = unsafe { opus_encoder_create(RATE as i32, CHANNELS as c_int, APPLICATION_AUDIO, &mut error) };
        if raw.is_null() {
            return Err(format!("libopus couldn't start an encoder (error {error})"));
        }
        let mut lookahead: c_int = 0;
        // SAFETY: `raw` is live; each request takes the one argument given.
        unsafe {
            opus_encoder_ctl(raw, SET_BITRATE, bitrate);
            opus_encoder_ctl(raw, GET_LOOKAHEAD, &mut lookahead as *mut c_int);
        }
        Ok(Self { raw, pre_skip: lookahead.clamp(0, i32::from(u16::MAX)) as u16 })
    }

    /// One 20 ms frame: `FRAME * CHANNELS` interleaved samples.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, String> {
        assert_eq!(pcm.len(), FRAME * CHANNELS, "a frame is 20 ms of stereo");
        let mut packet = vec![0u8; 4000];
        // SAFETY: `pcm` holds `FRAME` samples for each channel and `packet` is as long as it's told.
        let written = unsafe { opus_encode_float(self.raw, pcm.as_ptr(), FRAME as c_int, packet.as_mut_ptr(), packet.len() as i32) };
        if written < 0 {
            return Err(format!("libopus failed to encode audio (error {written})"));
        }
        packet.truncate(written as usize);
        Ok(packet)
    }

    /// The header the WebM track carries so a player can set its decoder up.
    pub fn head(&self) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(CHANNELS as u8);
        head.extend_from_slice(&self.pre_skip.to_le_bytes());
        head.extend_from_slice(&RATE.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        head
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `opus_encoder_create` and is freed once.
        unsafe { opus_encoder_destroy(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_becomes_a_small_packet_with_a_valid_header() {
        let mut encoder = Encoder::new(128_000).unwrap();
        let tone: Vec<f32> = (0..FRAME).flat_map(|n| {
            let s = (n as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.3;
            [s, s]
        }).collect();
        let packet = encoder.encode(&tone).unwrap();
        assert!(!packet.is_empty() && packet.len() < 1000, "{} bytes", packet.len());
        let head = encoder.head();
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(head.len(), 19);
        assert_eq!(head[9], 2);
        assert_eq!(u32::from_le_bytes(head[12..16].try_into().unwrap()), 48_000);
    }
}
