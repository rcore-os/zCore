#![no_main]
#![cfg_attr(not(feature = "libos"), no_std)]
#![feature(lang_items)]
#![feature(core_intrinsics)]
// #![deny(warnings)] // comment this on develop

use core::sync::atomic::{AtomicBool, Ordering};

extern crate alloc;
#[macro_use]
extern crate log;
#[macro_use]
extern crate cfg_if;

#[macro_use]
mod logging;

#[cfg(not(feature = "libos"))]
mod lang;

mod fs;
mod handler;
mod memory;
mod platform;
mod utils;

static STARTED: AtomicBool = AtomicBool::new(false);

fn primary_main(config: kernel_hal::KernelConfig) {
    logging::init();
    memory::init_heap();
    kernel_hal::primary_init_early(config, &handler::ZcoreKernelHandler);

    let options = utils::boot_options();
    logging::set_max_level(&options.log_level);
    info!("Boot options: {:#?}", options);
    memory::init_frame_allocator(&kernel_hal::mem::free_pmem_regions());
    kernel_hal::primary_init();
    STARTED.store(true, Ordering::SeqCst);

    cfg_if! {
        if #[cfg(all(feature = "linux", feature = "zircon"))] {
            panic!("Feature `linux` and `zircon` cannot be enabled at the same time!");
        } else if #[cfg(feature = "linux")] {
            log::info!("run prog");
            let args = options.root_proc.split('?').map(Into::into).collect(); // parse "arg0?arg1?arg2"
            let envs = alloc::vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin".into()];
            let rootfs = fs::rootfs();
            let proc = zcore_loader::linux::run(args, envs, rootfs);
            utils::wait_for_exit(Some(proc))
        } else if #[cfg(feature = "zircon")] {
            let zbi = fs::zbi();
            let proc = zcore_loader::zircon::run_userboot(zbi, &options.cmdline);
            utils::wait_for_exit(Some(proc))
        } else {
            panic!("One of the features `linux` or `zircon` must be specified!");
        }
    }
}

#[allow(dead_code)]
#[cfg(not(feature = "libos"))]
fn secondary_main() -> ! {
    while !STARTED.load(Ordering::SeqCst) {}
    // Don't print anything between previous line and next line.
    // Boot hart has initialized the UART chip, so we will use
    // UART for output instead of SBI, but the current HART is
    // not mapped to UART MMIO, which means we can't output
    // until secondary_init is complete.
    kernel_hal::secondary_init();
    log::info!("hart{} inited", kernel_hal::cpu::cpu_id());
    utils::wait_for_exit(None)
}
