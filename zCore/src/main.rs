#![no_std]
#![no_main]
#![feature(lang_items)]
#![feature(asm)]
#![feature(panic_info_message)]
#![feature(ptr_offset_from)]
#![deny(unused_must_use)]
#![deny(warnings)] // comment this on develop

extern crate alloc;
#[macro_use]
extern crate log;
extern crate rlibc;

#[macro_use]
mod logging;
mod lang;
mod memory;

use rboot::BootInfo;

use core::fmt::{Debug, Error, Formatter};
pub use memory::{hal_frame_alloc, hal_frame_dealloc, hal_pt_map_kernel};
use zircon_loader::{run_userboot, Images};
use zircon_object::util::kcounter::KCounterDesc;

#[no_mangle]
pub extern "C" fn _start(boot_info: &BootInfo) -> ! {
    logging::init(get_log_level(boot_info.cmdline));
    memory::init_heap();
    memory::init_frame_allocator(boot_info);
    #[cfg(feature = "graphic")]
    init_framebuffer(boot_info);
    info!("{:#x?}", boot_info);
    kernel_hal_bare::init();

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
    let images = Images::<&[u8]> {
        userboot: include_bytes!("../../prebuilt/zircon/userboot.so"),
        vdso: include_bytes!("../../prebuilt/zircon/libzircon.so"),
        decompressor: include_bytes!("../../prebuilt/zircon/decompress-zstd.so"),
        zbi: zbi_data,
    };
    let _proc = run_userboot(&images, cmdline);
    executor::run();
}

fn get_log_level(cmdline: &str) -> &str {
    for opt in cmdline.split(':') {
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

#[cfg(feature = "graphic")]
fn init_framebuffer(boot_info: &BootInfo) {
    let (width, height) = boot_info.graphic_info.mode.resolution();
    let fb_addr = boot_info.graphic_info.fb_addr as usize;
    kernel_hal_bare::init_framebuffer(width as u32, height as u32, fb_addr);
}

struct KCounterDescs(&'static [KCounterDesc]);

impl KCounterDescs {
    fn get() -> Self {
        extern "C" {
            fn _kcounter_desc_start();
            fn _kcounter_desc_end();
            fn _kcounter_start();
            fn _kcounter_end();
        }
        let start = _kcounter_desc_start as usize as *const KCounterDesc;
        let end = _kcounter_desc_end as usize as *const KCounterDesc;
        let descs = unsafe { core::slice::from_raw_parts(start, end.offset_from(start) as usize) };
        KCounterDescs(descs)
    }
}

impl Debug for KCounterDescs {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_str("KCounters ")?;
        f.debug_map()
            .entries(self.0.iter().map(|desc| (desc.name, desc.kcounter.get())))
            .finish()
    }
}
