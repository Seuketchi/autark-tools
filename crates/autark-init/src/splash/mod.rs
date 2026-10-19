//! The boot splash (docs/specs/boot-splash.md): a loading screen with the PIN box, drawn straight onto the firmware's
//! framebuffer. It is off unless the kernel command line says `autark.splash=1`, and any failure to draw gives the text prompt.

pub mod canvas;
pub mod console;
pub mod entry;
pub mod fb;
pub mod font;
pub mod scene;
