pub mod context;
pub mod cpu;
pub mod dev;
pub mod interrupt;
pub mod memory;
pub mod misc;
pub mod paging;
pub mod rand;
pub mod serial;
pub mod thread;
pub mod timer;
pub mod vdso;

pub use self::misc::*; // FIXME

/// Initialize the HAL.
///
/// This function must be called at the beginning.
pub fn init() {
    unimplemented!();
}
