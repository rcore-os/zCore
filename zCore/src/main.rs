#![no_std]
#![no_main]
#![feature(lang_items)]
#![feature(asm)]
#![feature(panic_info_message)]
#![deny(unused_must_use, unused_imports, warnings)]

extern crate alloc;
#[macro_use]
extern crate log;
extern crate rlibc;

#[macro_use]
mod logging;
mod interrupt;
mod lang;
mod memory;
mod process;

use {kernel_hal_bare::arch::timer_init, rboot::BootInfo};

pub use memory::{hal_frame_alloc, hal_frame_dealloc, hal_pt_map_kernel};

#[no_mangle]
pub extern "C" fn _start(boot_info: &BootInfo) -> ! {
    logging::init(get_log_level(boot_info.cmdline));
    memory::init_heap();
    memory::init_frame_allocator(boot_info);
    info!("{:#x?}", boot_info);
    interrupt::init();
    timer_init();
    process::init();
    unreachable!();
}

fn get_log_level(cmdline: &str) -> &str {
    for opt in cmdline.split(',') {
        // parse 'key=value'
        let mut iter = opt.trim().splitn(2, '=');
        let key = iter.next().expect("failed to parse key");
        let value = iter.next().expect("failed to parse value");
        if key == "LOG" {
            return value;
        }
    }
    ""
}
