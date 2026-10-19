//! What the machine has and what runs it, read from the kernel's own records (udev's database, `modules.alias`,
//! the module list and the kernel log) rather than a table of known hardware, so a machine nobody has seen yet is
//! described as well as one everybody has.

pub mod aliases;
pub mod controls;
pub mod devices;
pub mod doctor;
pub mod hotplug;
pub mod kernel_log;
