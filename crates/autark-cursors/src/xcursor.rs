//! The Xcursor file format: a header, a table of images, then each image as a header and premultiplied ARGB pixels.

/// One image of a cursor, at one nominal size. Animated cursors have several per size, in order.
pub struct Image {
    pub size: u32,
    pub width: u32,
    pub height: u32,
    pub hot: (u32, u32),
    /// Milliseconds this frame shows for.
    pub delay: u32,
    /// Premultiplied ARGB, one little-endian `u32` per pixel.
    pub pixels: Vec<u8>,
}

const IMAGE: u32 = 0xfffd_0002;

pub fn encode(images: &[Image]) -> Vec<u8> {
    let mut out = Vec::new();
    let word = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(b"Xcur");
    word(&mut out, 16);
    word(&mut out, 0x1_0000);
    word(&mut out, images.len() as u32);
    let mut position = 16 + 12 * images.len() as u32;
    for image in images {
        word(&mut out, IMAGE);
        word(&mut out, image.size);
        word(&mut out, position);
        position += 36 + image.pixels.len() as u32;
    }
    for image in images {
        for value in [36, IMAGE, image.size, 1, image.width, image.height, image.hot.0, image.hot.1, image.delay] {
            word(&mut out, value);
        }
        out.extend_from_slice(&image.pixels);
    }
    out
}
