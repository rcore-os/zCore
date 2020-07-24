#![no_std]
#![no_main]
#![feature(lang_items)]
#![feature(asm)]
#![feature(panic_info_message)]
#![feature(global_asm)]
// #![deny(unused_must_use)]
// #![deny(warnings)] // comment this on develop

extern crate alloc;
#[macro_use]
extern crate log;
extern crate rlibc;
// extern crate rlibc_opt;

#[macro_use]
mod logging;
mod lang;
mod memory;

#[cfg(target_arch = "x86_64")]
use rboot::BootInfo;

pub use memory::{hal_frame_alloc, hal_frame_dealloc, hal_pt_map_kernel};

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn _start(boot_info: &BootInfo) -> ! {
    logging::init(get_log_level(boot_info.cmdline));
    memory::init_heap();
    memory::init_frame_allocator(boot_info);
    #[cfg(feature = "graphic")]
    init_framebuffer(boot_info);
    info!("{:#x?}", boot_info);
    kernel_hal_bare::init(kernel_hal_bare::Config {
        acpi_rsdp: boot_info.acpi2_rsdp_addr,
        smbios: boot_info.smbios_addr,
        ap_fn: run,
    });

    let ramfs_data = unsafe {
        core::slice::from_raw_parts_mut(
            (boot_info.initramfs_addr + boot_info.physical_memory_offset) as *mut u8,
            boot_info.initramfs_size as usize,
        )
    };
    main(ramfs_data, boot_info.cmdline);
    unreachable!();
}

#[cfg(target_arch = "mips")]
global_asm!(include_str!("arch/mipsel/boot/entry.gen.s"));

#[cfg(target_arch = "mips")]
use mips::registers::cp0;

#[cfg(target_arch = "mips")]
#[path = "arch/mipsel/board/malta/mod.rs"]
pub mod board;

#[cfg(target_arch = "mips")]
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    let ebase = cp0::ebase::read_u32();
    let cpu_id = ebase & 0x3ff;
    let dtb_start = board::DTB.as_ptr() as usize;
    if cpu_id != 0 {
        // TODO: run others_main on other CPU
        // while unsafe { !cpu::has_started(hartid) }  { }
        // others_main();
        loop {}
    }
    // unsafe { cpu::set_cpu_id(hartid); }
    logging::init("info");
    kernel_hal_bare::init(kernel_hal_bare::Config {});
    // loop {}
    // drivers::uart16550::driver_init();
    unimplemented!();
    // let ebase = cp0::ebase::read_u32();
    // let cpu_id = ebase & 0x3ff;
    // let dtb_start = board::DTB.as_ptr() as usize;
    // const BOOT_CPU_ID: u32 = 0;

    // unsafe {
    //     memory::clear_bss();
    // }

    // board::early_init();
    // crate::logging::init();

    // interrupt::init();
    // memory::init_heap();
    // memory::init_frame_allocator();
    // timer::init();
    // board::init(dtb_start);

    // info!("Hello MIPS 32 from CPU {}, dtb @ {:#x}", cpu_id, dtb_start);
    info!("Hello MIPS 32");

    //crate::drivers::init(dtb_start);
    // crate::process::init();

    // TODO: start other CPU
    // unsafe { cpu::start_others(hart_mask); }
    // main(ramfs_data, boot_info.cmdline);
    unreachable!();
}

#[cfg(feature = "zircon")]
fn main(ramfs_data: &[u8], cmdline: &str) {
    use zircon_loader::{run_userboot, Images};
    let images = Images::<&[u8]> {
        userboot: include_bytes!("../../prebuilt/zircon/x64/userboot.so"),
        vdso: include_bytes!("../../prebuilt/zircon/x64/libzircon.so"),
        zbi: ramfs_data,
    };
    let _proc = run_userboot(&images, cmdline);
    run();
}

#[cfg(feature = "linux")]
fn main(ramfs_data: &'static mut [u8], _cmdline: &str) {
    use alloc::sync::Arc;
    use alloc::vec;
    use linux_object::fs::MemBuf;

    let args = vec!["/bin/busybox".into()];
    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/x86_64-alpine-linux-musl/bin".into()];

    let device = Arc::new(MemBuf::new(ramfs_data));
    let rootfs = rcore_fs_sfs::SimpleFileSystem::open(device).unwrap();
    let _proc = linux_loader::run(args, envs, rootfs);
    run();
}

fn run() -> ! {
    loop {
        executor::run_until_idle();
        kernel_hal_bare::arch::wait_for_interrupt();
        // x86_64::instructions::interrupts::enable_interrupts_and_hlt();
        // x86_64::instructions::interrupts::disable();
    }
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
