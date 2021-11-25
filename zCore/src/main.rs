#![no_main]
#![cfg_attr(not(feature = "libos"), no_std)]
#![feature(global_asm)]
#![feature(lang_items)]
#![feature(asm)]
#![deny(warnings)] // comment this on develop

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

fn primary_main(config: kernel_hal::KernelConfig) {
    logging::init();
    memory::init_heap();
    kernel_hal::primary_init_early(config, &handler::ZcoreKernelHandler);

    let options = utils::boot_options();
    logging::set_max_level(&options.log_level);
    info!("Boot options: {:#?}", options);

    memory::init_frame_allocator(&kernel_hal::mem::free_pmem_regions());
    kernel_hal::primary_init();

    // for i in 0..=1 {
    //     if i != kernel_hal::cpu::cpu_id() {
    //         let ipi_mask = 1usize << i;
    //         warn!("send ipi to {}, mask={}", i, ipi_mask);
    //         kernel_hal::arch::sbi::send_ipi(&ipi_mask as *const usize as usize);
    //         warn!("send ipi to {}, finish", i);
    //     }
    // }
    cfg_if! {
        if #[cfg(all(feature = "linux", feature = "zircon"))] {
            panic!("Feature `linux` and `zircon` cannot be enabled at the same time!");
        } else if #[cfg(feature = "linux")] {
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
fn secondary_main() -> ! {
    kernel_hal::secondary_init();
    utils::wait_for_exit(None)
}
