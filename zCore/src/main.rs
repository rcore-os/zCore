#![no_std]
#![no_main]
#![feature(lang_items)]
#![feature(asm)]
#![feature(panic_info_message)]
#![deny(unused_must_use)]
#![deny(warnings)] // comment this on develop

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

use rboot::BootInfo;

pub use memory::{hal_frame_alloc, hal_frame_dealloc, hal_pt_map_kernel};
use zircon_loader::run_userboot;

#[no_mangle]
pub extern "C" fn _start(boot_info: &BootInfo) -> ! {
    logging::init(get_log_level(boot_info.cmdline));
    memory::init_heap();
    memory::init_frame_allocator(boot_info);
    info!("{:#x?}", boot_info);
    kernel_hal_bare::init();
    interrupt::init();
    process::init();

    let zbi_data = unsafe {
        core::slice::from_raw_parts(
            (boot_info.initramfs_addr + boot_info.physical_memory_offset) as *const u8,
            boot_info.initramfs_size as usize,
        )
    };
    main(zbi_data, boot_info.cmdline);
    unreachable!();
}

fn main(zbi_data: &[u8], cmdline: &str) {
    let _proc = run_userboot(
        USERBOOT_DATA,
        VDSO_DATA,
        DECOMPRESSOR_DATA,
        &zbi_data,
        cmdline,
    );
    executor::run();
}

static USERBOOT_DATA: &[u8] = include_bytes!("../../prebuilt/zircon/userboot.so");
static VDSO_DATA: &[u8] = include_bytes!("../../prebuilt/zircon/libzircon.so");
static DECOMPRESSOR_DATA: &[u8] = include_bytes!("../../prebuilt/zircon/decompress-zstd.so");

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
