mod mem_common;

pub mod context;
pub mod cpu;
pub mod memory;
pub mod paging;
pub mod serial;
pub mod thread;
pub mod timer;
pub mod vdso;

#[path = "../unimp/interrupt.rs"]
pub mod interrupt;
#[path = "../unimp/rand.rs"]
pub mod rand;

cfg_if::cfg_if! {
    if #[cfg(target_os = "linux")] {
        pub mod dev;
    } else {
        #[path = "../unimp/dev/mod.rs"]
        pub mod dev;
    }
}

#[cfg(target_os = "macos")]
include!("macos.rs");

/// Initialize the HAL.
///
/// This function must be called at the beginning.
pub fn init() {
    #[cfg(target_os = "macos")]
    unsafe {
        register_sigsegv_handler();
    }
    // spawn a thread to read stdin
    // TODO: raw mode
    use std::io::Read;
    std::thread::spawn(|| {
        for i in std::io::stdin().bytes() {
            serial::serial_put(i.unwrap());
        }
    });
}

// FIXME
#[path = "../unimp/misc.rs"]
mod misc;
pub use misc::*;
