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
    info!(
        "SFS linked to kernel, from {:08x} to {:08x}",
        boot_info.initramfs_addr as usize + boot_info.physical_memory_offset as usize,
        boot_info.initramfs_addr as usize
            + boot_info.physical_memory_offset as usize
            + boot_info.initramfs_size as usize
    );
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

// Hard link user programs
#[cfg(feature = "link_user")]
global_asm!(concat!(
    r#"
	.section .data.img
	.global _user_img_start
	.global _user_img_end
_user_img_start:
    .incbin ""#,
    env!("USER_IMG"),
    r#""
_user_img_end:
"#
));

#[cfg(target_arch = "mips")]
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    let ebase = cp0::ebase::read_u32();
    let cpu_id = ebase & 0x3ff;
    let dtb_start = board::DTB.as_ptr() as usize;
    const BOOT_CPU_ID: u32 = 0;
    if cpu_id != BOOT_CPU_ID {
        // TODO: run others_main on other CPU
        // while unsafe { !cpu::has_started(hartid) }  { }
        // others_main();
        loop {}
    }
    // unsafe { cpu::set_cpu_id(hartid); }
    unsafe {
        memory::clear_bss();
    }
    logging::init("info");
    memory::init_heap();
    memory::init_frame_allocator();
    kernel_hal_bare::init(kernel_hal_bare::Config {});
    board::init(dtb_start);
    info!("Hello MIPS 32 from CPU {}, dtb @ {:#x}", cpu_id, dtb_start);
    extern "C" {
        fn _user_img_start();
        fn _user_img_end();
    }
    use core::slice;
    let ramfs_data = unsafe {
        slice::from_raw_parts_mut(
            _user_img_start as *mut u8,
            _user_img_end as usize - _user_img_start as usize,
        )
    };
    info!(
        "SFS linked to kernel, from {:08x} to {:08x}",
        _user_img_start as usize, _user_img_end as usize
    );
    main(ramfs_data, "");
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

    #[cfg(target_arch = "x86_64")]
    let args = vec!["/bin/busybox".into()];
    #[cfg(target_arch = "mips")]
    let args = vec!["busybox".into()];

    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/x86_64-alpine-linux-musl/bin".into()];

    let device = Arc::new(MemBuf::new(ramfs_data));
    let rootfs = rcore_fs_sfs::SimpleFileSystem::open(device).unwrap();
    let _proc = linux_loader::run(args, envs, rootfs);
    run();
}

fn run() -> ! {
    loop {
        executor::run_until_idle();
        kernel_hal::InterruptManager::wait_for_interrupt();
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
