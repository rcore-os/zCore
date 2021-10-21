#![no_std]
#![no_main]
#![feature(global_asm)]
#![feature(lang_items)]
#![deny(warnings)] // comment this on develop

extern crate alloc;
#[macro_use]
extern crate log;
#[macro_use]
extern crate cfg_if;

#[macro_use]
mod logging;

mod fs;
mod handler;
mod lang;
mod memory;
mod platform;
mod utils;

fn primary_main(config: kernel_hal::KernelConfig) {
    logging::init();
    memory::init_heap();
    kernel_hal::primary_init_early(config, &handler::ZcoreKernelHandler);

    let cmdline = kernel_hal::boot::cmdline();
    let boot_options = utils::parse_cmdline(cmdline);
    println!("Boot options: {:#?}", boot_options);
    logging::set_max_level(boot_options.get("LOG").unwrap_or(&""));

    memory::init_frame_allocator(&kernel_hal::mem::free_pmem_regions());
    kernel_hal::primary_init();

    cfg_if! {
        if #[cfg(feature = "linux")] {
            let args = boot_options
                .get("ROOTPROC").unwrap_or(&"/bin/busybox?sh")
                .split('?').map(Into::into).collect(); // parse "arg0?arg1?arg2"
            let envs = alloc::vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin".into()];
            let rootfs = fs::rootfs();
            linux_loader::run(args, envs, rootfs);
        } else {
            let images = zircon_loader::Images::<&[u8]> {
                userboot: include_bytes!("../../prebuilt/zircon/x64/userboot.so"),
                vdso: include_bytes!("../../prebuilt/zircon/x64/libzircon.so"),
                zbi: fs::init_ram_disk(),
            };
            zircon_loader::run_userboot(&images, cmdline);
        }
    }
    utils::run_tasks_forever()
}

#[allow(dead_code)]
fn secondary_main() -> ! {
    kernel_hal::secondary_init();
    utils::run_tasks_forever()
}
