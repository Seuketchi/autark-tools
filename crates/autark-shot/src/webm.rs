//! A WebM file with a VP8 video track and, if asked, an Opus audio track: the EBML header, an info block whose duration
//! is filled in when the recording ends, the tracks, clusters of frames and a seek index. Times are in milliseconds.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

/// A cluster's frame times are 16 bits relative to its start; this leaves room.
const CLUSTER_MS: i64 = 30_000;

pub struct Writer {
    file: File,
    /// Where the segment's contents begin, which seek-index positions count from.
    segment: u64,
    /// Where the info block's 8-byte duration is, to be written once it's known.
    duration_at: u64,
    cluster: Option<Cluster>,
    /// Start time and position of each cluster that begins with a key frame.
    cues: Vec<(i64, u64)>,
    last: i64,
}

struct Cluster {
    start: i64,
    key: bool,
    blocks: Vec<u8>,
}

impl Writer {
    /// `audio` is an Opus header (see `opus::Encoder::head`) and its pre-skip, if there's a second track.
    pub fn create(path: &Path, width: u32, height: u32, audio: Option<(&[u8], u16)>) -> io::Result<Self> {
        let mut file = File::create(path)?;
        let mut header = Vec::new();
        let mut ebml = Vec::new();
        uint(&mut ebml, &[0x42, 0x86], 1);
        uint(&mut ebml, &[0x42, 0xF7], 1);
        uint(&mut ebml, &[0x42, 0xF2], 4);
        uint(&mut ebml, &[0x42, 0xF3], 8);
        text(&mut ebml, &[0x42, 0x82], "webm");
        uint(&mut ebml, &[0x42, 0x87], 4);
        uint(&mut ebml, &[0x42, 0x85], 2);
        master(&mut header, &[0x1A, 0x45, 0xDF, 0xA3], &ebml);
        // A segment of unknown length, as a recording that may be cut short needs.
        header.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        let segment = header.len() as u64;

        let mut info = Vec::new();
        uint(&mut info, &[0x2A, 0xD7, 0xB1], 1_000_000);
        info.extend_from_slice(&[0x44, 0x89, 0x88]);
        let duration_in_info = info.len() as u64;
        info.extend_from_slice(&0f64.to_be_bytes());
        text(&mut info, &[0x4D, 0x80], "autark-shot");
        text(&mut info, &[0x57, 0x41], "autark-shot");
        // The info block's own id and 8-byte size come first.
        let duration_at = header.len() as u64 + 4 + 8 + duration_in_info;
        master(&mut header, &[0x15, 0x49, 0xA9, 0x66], &info);

        let mut video = Vec::new();
        uint(&mut video, &[0xB0], width.into());
        uint(&mut video, &[0xBA], height.into());
        let mut track = Vec::new();
        uint(&mut track, &[0xD7], 1);
        uint(&mut track, &[0x73, 0xC5], 1);
        uint(&mut track, &[0x83], 1);
        text(&mut track, &[0x86], "V_VP8");
        master(&mut track, &[0xE0], &video);
        let mut tracks = Vec::new();
        master(&mut tracks, &[0xAE], &track);
        if let Some((head, pre_skip)) = audio {
            let mut sound = Vec::new();
            sound.extend_from_slice(&[0xB5, 0x88]);
            sound.extend_from_slice(&48_000f64.to_be_bytes());
            uint(&mut sound, &[0x9F], 2);
            let mut entry = Vec::new();
            uint(&mut entry, &[0xD7], 2);
            uint(&mut entry, &[0x73, 0xC5], 2);
            uint(&mut entry, &[0x83], 2);
            text(&mut entry, &[0x86], "A_OPUS");
            master(&mut entry, &[0x63, 0xA2], head);
            uint(&mut entry, &[0x56, 0xAA], u64::from(pre_skip) * 1_000_000_000 / 48_000);
            uint(&mut entry, &[0x56, 0xBB], 80_000_000);
            master(&mut entry, &[0xE1], &sound);
            master(&mut tracks, &[0xAE], &entry);
        }
        master(&mut header, &[0x16, 0x54, 0xAE, 0x6B], &tracks);

        file.write_all(&header)?;
        Ok(Self { file, segment, duration_at, cluster: None, cues: Vec::new(), last: 0 })
    }

    /// Adds a video frame shown at `pts` milliseconds. A key frame starts a new cluster, which the seek index points at.
    pub fn frame(&mut self, pts: i64, key: bool, data: &[u8]) -> io::Result<()> {
        self.block(1, pts, key, data)
    }

    /// Adds a packet of audio at `pts` milliseconds. Every Opus packet stands alone, and none starts a cluster.
    pub fn sound(&mut self, pts: i64, data: &[u8]) -> io::Result<()> {
        self.block(2, pts, false, data)
    }

    fn block(&mut self, track: u8, pts: i64, key: bool, data: &[u8]) -> io::Result<()> {
        let starts = track == 1 && key;
        if self.cluster.as_ref().is_some_and(|cluster| starts || pts - cluster.start > CLUSTER_MS) {
            self.flush_cluster()?;
        }
        let cluster = self.cluster.get_or_insert_with(|| Cluster { start: pts, key: starts, blocks: Vec::new() });
        let mut block = vec![0x80 | track];
        block.extend_from_slice(&((pts - cluster.start) as i16).to_be_bytes());
        block.push(if key || track == 2 { 0x80 } else { 0 });
        block.extend_from_slice(data);
        master(&mut cluster.blocks, &[0xA3], &block);
        self.last = self.last.max(pts);
        Ok(())
    }

    fn flush_cluster(&mut self) -> io::Result<()> {
        let Some(cluster) = self.cluster.take() else { return Ok(()) };
        let position = self.file.stream_position()? - self.segment;
        if cluster.key {
            self.cues.push((cluster.start, position));
        }
        let mut contents = Vec::new();
        uint(&mut contents, &[0xE7], cluster.start as u64);
        contents.extend_from_slice(&cluster.blocks);
        let mut out = Vec::new();
        master(&mut out, &[0x1F, 0x43, 0xB6, 0x75], &contents);
        self.file.write_all(&out)
    }

    /// Writes what's left, the seek index and the duration.
    pub fn finish(mut self, frame_ms: i64) -> io::Result<()> {
        self.flush_cluster()?;
        let mut points = Vec::new();
        for (time, position) in &self.cues {
            let mut place = Vec::new();
            uint(&mut place, &[0xF7], 1);
            uint(&mut place, &[0xF1], *position);
            let mut point = Vec::new();
            uint(&mut point, &[0xB3], *time as u64);
            master(&mut point, &[0xB7], &place);
            master(&mut points, &[0xBB], &point);
        }
        let mut cues = Vec::new();
        master(&mut cues, &[0x1C, 0x53, 0xBB, 0x6B], &points);
        self.file.write_all(&cues)?;
        self.file.seek(SeekFrom::Start(self.duration_at))?;
        self.file.write_all(&((self.last + frame_ms) as f64).to_be_bytes())?;
        self.file.flush()
    }
}

/// An element: its id, its size in a fixed 8 bytes, then its contents.
fn master(out: &mut Vec<u8>, id: &[u8], contents: &[u8]) {
    out.extend_from_slice(id);
    out.push(0x01);
    out.extend_from_slice(&(contents.len() as u64).to_be_bytes()[1..]);
    out.extend_from_slice(contents);
}

fn uint(out: &mut Vec<u8>, id: &[u8], value: u64) {
    let bytes = value.to_be_bytes();
    let skip = bytes.iter().take(7).take_while(|byte| **byte == 0).count();
    out.extend_from_slice(id);
    out.push(0x80 | (8 - skip) as u8);
    out.extend_from_slice(&bytes[skip..]);
}

fn text(out: &mut Vec<u8>, id: &[u8], value: &str) {
    out.extend_from_slice(id);
    out.push(0x80 | value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|window| window == needle)
    }

    #[test]
    fn integers_use_the_fewest_bytes() {
        let mut out = Vec::new();
        uint(&mut out, &[0xB0], 0);
        uint(&mut out, &[0xB0], 1920);
        uint(&mut out, &[0xB0], 1_000_000);
        assert_eq!(out, [0xB0, 0x81, 0, 0xB0, 0x82, 0x07, 0x80, 0xB0, 0x83, 0x0F, 0x42, 0x40]);
    }

    #[test]
    fn a_recording_has_its_parts_and_its_duration() {
        let path = std::env::temp_dir().join(format!("autark-shot-webm-{}.webm", std::process::id()));
        let mut writer = Writer::create(&path, 640, 480, Some((b"OpusHead-test", 312))).unwrap();
        writer.frame(0, true, b"key-one").unwrap();
        writer.sound(10, b"audio-one").unwrap();
        writer.frame(33, false, b"delta").unwrap();
        writer.frame(66, true, b"key-two").unwrap();
        writer.finish(33).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bytes[..4], [0x1A, 0x45, 0xDF, 0xA3]);
        for part in [&b"webm"[..], b"V_VP8", b"A_OPUS", b"OpusHead-test", b"audio-one", b"key-one", b"delta", b"key-two"] {
            assert!(find(&bytes, part).is_some(), "{}", String::from_utf8_lossy(part));
        }
        // Two clusters, as the second key frame starts one, and a seek index with a point for each.
        let clusters = bytes.windows(4).filter(|window| *window == [0x1F, 0x43, 0xB6, 0x75]).count();
        assert_eq!(clusters, 2);
        assert!(find(&bytes, &[0x1C, 0x53, 0xBB, 0x6B]).is_some());
        // The duration was patched in after the info block's id and size: 66 + 33 ms.
        let info = find(&bytes, &[0x15, 0x49, 0xA9, 0x66]).unwrap();
        let at = find(&bytes[info..], &[0x44, 0x89, 0x88]).unwrap() + info + 3;
        assert_eq!(f64::from_be_bytes(bytes[at..at + 8].try_into().unwrap()), 99.0);
    }
}
