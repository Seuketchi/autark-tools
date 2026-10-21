//! Reading a `.wav` file into mono samples, for sound sets: PCM at 8, 16, 24 or 32 bits, or 32-bit float, any rate and any number of
//! channels. Anything longer than a few seconds is refused, since these are short effects.

/// The longest effect read, in seconds.
pub const MAX_SECONDS: f32 = 5.0;

fn le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// The file as mono samples between -1 and 1, resampled to `rate`.
pub fn decode(bytes: &[u8], rate: u32) -> Result<Vec<f32>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".to_owned());
    }
    let (mut format, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let (id, size) = (&bytes[at..at + 4], le32(&bytes[at + 4..]) as usize);
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let mut tag = le16(body);
                // "Extensible" keeps the real format in the first two bytes of its sub-format.
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = le16(&body[24..]);
                }
                format = Some((tag, le16(&body[2..]) as usize, le32(&body[4..]), le16(&body[14..]) as usize));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are padded to an even size.
        at += 8 + size + (size & 1);
    }
    let ((tag, channels, source_rate, bits), data) = format.zip(data).ok_or("it has no sound in it")?;
    if channels == 0 || source_rate == 0 {
        return Err("it has no channels or no rate".to_owned());
    }
    let width = bits / 8;
    let sample = |chunk: &[u8]| -> Option<f32> {
        Some(match (tag, bits) {
            (1, 8) => (f32::from(chunk[0]) - 128.0) / 128.0,
            (1, 16) => f32::from(i16::from_le_bytes([chunk[0], chunk[1]])) / 32768.0,
            (1, 24) => (i32::from_le_bytes([0, chunk[0], chunk[1], chunk[2]]) >> 8) as f32 / 8_388_608.0,
            (1, 32) => i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f32 / 2_147_483_648.0,
            (3, 32) => f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            _ => return None,
        })
    };
    if !matches!((tag, bits), (1, 8 | 16 | 24 | 32) | (3, 32)) {
        return Err(format!("format {tag} at {bits} bits isn't supported (use 16-bit PCM)"));
    }
    let frames = data.len() / (width * channels);
    if frames as f32 / source_rate as f32 > MAX_SECONDS {
        return Err(format!("it is longer than {MAX_SECONDS} seconds"));
    }
    let mono: Vec<f32> = data
        .chunks_exact(width * channels)
        .map(|frame| frame.chunks_exact(width).filter_map(sample).sum::<f32>() / channels as f32)
        .collect();
    if mono.is_empty() {
        return Err("it has no sound in it".to_owned());
    }
    if source_rate == rate {
        return Ok(mono);
    }
    let length = (mono.len() as f64 * f64::from(rate) / f64::from(source_rate)) as usize;
    Ok((0..length)
        .map(|index| {
            let at = index as f64 * f64::from(source_rate) / f64::from(rate);
            let (before, fraction) = (at as usize, (at - at.floor()) as f32);
            let after = (before + 1).min(mono.len() - 1);
            mono[before.min(mono.len() - 1)] * (1.0 - fraction) + mono[after] * fraction
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file of 16-bit PCM.
    fn wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        let mut file = b"RIFF".to_vec();
        file.extend((36 + data.len() as u32).to_le_bytes());
        file.extend(b"WAVEfmt ");
        file.extend(16_u32.to_le_bytes());
        file.extend(1_u16.to_le_bytes());
        file.extend(channels.to_le_bytes());
        file.extend(rate.to_le_bytes());
        file.extend((rate * u32::from(channels) * 2).to_le_bytes());
        file.extend((channels * 2).to_le_bytes());
        file.extend(16_u16.to_le_bytes());
        file.extend(b"data");
        file.extend((data.len() as u32).to_le_bytes());
        file.extend(data);
        file
    }

    #[test]
    fn mono_16_bit_reads_as_it_is() {
        let samples = decode(&wav(1, 48_000, &[0, 16384, -16384, 32767]), 48_000).unwrap();
        assert_eq!(samples.len(), 4);
        assert!((samples[1] - 0.5).abs() < 1e-4 && (samples[2] + 0.5).abs() < 1e-4 && samples[3] > 0.99);
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let samples = decode(&wav(2, 48_000, &[16384, 0, -16384, -16384]), 48_000).unwrap();
        assert_eq!(samples.len(), 2);
        assert!((samples[0] - 0.25).abs() < 1e-4 && (samples[1] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn another_rate_is_resampled() {
        let samples = decode(&wav(1, 24_000, &[0, 1000, 2000, 3000]), 48_000).unwrap();
        assert_eq!(samples.len(), 8);
        // Halfway between the first two samples.
        assert!((samples[1] - 500.0 / 32768.0).abs() < 1e-4, "{}", samples[1]);
    }

    #[test]
    fn what_is_not_a_short_wav_is_refused() {
        assert!(decode(b"not a wav at all", 48_000).is_err());
        assert!(decode(&wav(1, 8_000, &vec![0; 8_000 * 6]), 48_000).unwrap_err().contains("longer than"));
        assert!(decode(&wav(1, 48_000, &[]), 48_000).is_err());
        let mut truncated = wav(1, 48_000, &[1, 2, 3, 4]);
        truncated.truncate(30);
        assert!(decode(&truncated, 48_000).is_err());
    }
}
