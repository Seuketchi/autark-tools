//! The firmware's framebuffer (`/dev/fb0`: simpledrm or efifb, which the kernel has before any graphics driver loads), mapped
//! into memory. Only 32-bit pixels are handled; anything else is "no splash", and the text prompt is used.

use std::fs::OpenOptions;
use std::os::fd::OwnedFd;

use rustix::ioctl::{Getter, ioctl};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};

use super::canvas::Canvas;

const FBIOGET_VSCREENINFO: u32 = 0x4600;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Bitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

/// The kernel's `struct fb_var_screeninfo`, 160 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VarScreenInfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: Bitfield,
    green: Bitfield,
    blue: Bitfield,
    transp: Bitfield,
    // nonstd, activate, height, width, accel_flags, the eleven timing fields and four reserved words.
    rest: [u32; 20],
}

/// How the pixels sit in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    /// Bytes from one row to the next.
    pub stride: usize,
    /// Rows to skip at the top (the visible part can start below row 0).
    pub y_offset: usize,
    /// Bit positions of red, green and blue in each 32-bit pixel.
    pub shifts: (u32, u32, u32),
}

impl Layout {
    pub fn bytes(&self) -> usize {
        (self.height + self.y_offset) * self.stride
    }
}

/// Copies the canvas into the framebuffer's memory, converting to its channel order. Anything the canvas doesn't cover is left as it is.
pub fn write_pixels(memory: &mut [u8], layout: &Layout, canvas: &Canvas) {
    let (rs, gs, bs) = layout.shifts;
    let width = layout.width.min(canvas.width);
    for y in 0..layout.height.min(canvas.height) {
        let start = (y + layout.y_offset) * layout.stride;
        let Some(row) = memory.get_mut(start..start + width * 4) else { return };
        for (x, out) in row.chunks_exact_mut(4).enumerate() {
            let p = canvas.pixels[y * canvas.width + x];
            let value = (((p >> 16) & 0xff) << rs) | (((p >> 8) & 0xff) << gs) | ((p & 0xff) << bs);
            out.copy_from_slice(&value.to_le_bytes());
        }
    }
}

pub struct Framebuffer {
    pub layout: Layout,
    base: *mut u8,
    length: usize,
    _file: OwnedFd,
}

// The mapping is only ever used by the one thread that owns the Framebuffer.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    pub fn open() -> Option<Framebuffer> {
        let file: OwnedFd = OpenOptions::new().read(true).write(true).open("/dev/fb0").ok()?.into();
        // SAFETY: FBIOGET_VSCREENINFO (the bare number 0x4600, not an `_IOR` one) fills a `struct fb_var_screeninfo`, which `VarScreenInfo` matches.
        let info: VarScreenInfo = unsafe { ioctl(&file, Getter::<FBIOGET_VSCREENINFO, VarScreenInfo>::new()) }.ok()?;
        if info.bits_per_pixel != 32 || info.xres == 0 || info.yres == 0 {
            return None;
        }
        let stride: usize = std::fs::read_to_string("/sys/class/graphics/fb0/stride").ok()?.trim().parse().ok()?;
        let layout = Layout {
            width: info.xres as usize,
            height: info.yres as usize,
            stride,
            y_offset: info.yoffset as usize,
            shifts: (info.red.offset, info.green.offset, info.blue.offset),
        };
        if stride < layout.width * 4 {
            return None;
        }
        let length = layout.bytes();
        // SAFETY: a shared read-write mapping of the device, unmapped in Drop; nothing else holds a reference into it.
        let base = unsafe { mmap(std::ptr::null_mut(), length, ProtFlags::READ | ProtFlags::WRITE, MapFlags::SHARED, &file, 0) }.ok()?;
        Some(Framebuffer { layout, base: base.cast(), length, _file: file })
    }

    pub fn present(&mut self, canvas: &Canvas) {
        // SAFETY: `base` maps `length` bytes for as long as `self` lives, and `&mut self` makes this the only user.
        let memory = unsafe { std::slice::from_raw_parts_mut(self.base, self.length) };
        write_pixels(memory, &self.layout, canvas);
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        // SAFETY: the mapping `open` made, not used again.
        unsafe {
            let _ = munmap(self.base.cast(), self.length);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(shifts: (u32, u32, u32)) -> Layout {
        Layout { width: 4, height: 2, stride: 20, y_offset: 0, shifts }
    }

    #[test]
    fn a_pixel_lands_in_the_channel_order_the_screen_asks_for() {
        let mut canvas = Canvas::new(4, 2);
        canvas.clear(0x112233);
        let mut memory = vec![0u8; 40];
        write_pixels(&mut memory, &layout((16, 8, 0)), &canvas);
        assert_eq!(&memory[0..4], &[0x33, 0x22, 0x11, 0x00]);
        let mut swapped = vec![0u8; 40];
        write_pixels(&mut swapped, &layout((0, 8, 16)), &canvas);
        assert_eq!(&swapped[0..4], &[0x11, 0x22, 0x33, 0x00]);
    }

    #[test]
    fn rows_start_a_stride_apart_and_the_padding_is_left_alone() {
        let mut canvas = Canvas::new(4, 2);
        canvas.clear(0xffffff);
        let mut memory = vec![0xAAu8; 40];
        write_pixels(&mut memory, &layout((16, 8, 0)), &canvas);
        assert_eq!(&memory[16..20], &[0xAA; 4], "the row's padding was written");
        assert_eq!(&memory[20..24], &[0xff, 0xff, 0xff, 0x00], "the second row starts at the stride");
    }

    #[test]
    fn a_visible_area_below_row_zero_is_respected() {
        let mut canvas = Canvas::new(4, 1);
        canvas.clear(0xffffff);
        let shifted = Layout { width: 4, height: 1, stride: 16, y_offset: 2, shifts: (16, 8, 0) };
        let mut memory = vec![0u8; shifted.bytes()];
        write_pixels(&mut memory, &shifted, &canvas);
        assert!(memory[..32].iter().all(|&b| b == 0));
        assert_eq!(&memory[32..36], &[0xff, 0xff, 0xff, 0x00]);
    }

    #[test]
    fn a_canvas_bigger_than_the_screen_or_memory_too_short_doesnt_panic() {
        let canvas = Canvas::new(100, 100);
        write_pixels(&mut [0u8; 40], &layout((16, 8, 0)), &canvas);
        write_pixels(&mut [0u8; 10], &layout((16, 8, 0)), &canvas);
    }

    #[test]
    fn the_var_screeninfo_is_the_size_the_kernel_expects() {
        assert_eq!(std::mem::size_of::<VarScreenInfo>(), 160);
    }
}
