#![no_std]
#![no_main]
#![feature(lang_items)]
#![feature(llvm_asm)]
#![feature(panic_info_message)]
#![deny(unused_must_use)]
#![feature(global_asm)]
/*
#![deny(warnings)] // comment this on develop
*/

extern crate alloc;
#[macro_use]
extern crate log;

#[cfg(target_arch = "riscv64")]
extern crate rlibc;

#[cfg(target_arch = "x86_64")]
extern crate rlibc_opt; //Only for x86_64

#[macro_use]
mod logging;
mod arch;
mod lang;
mod memory;

#[cfg(feature = "linux")]
mod fs;

#[cfg(target_arch = "x86_64")]
use rboot::BootInfo;

use kernel_hal::{KernelConfig, KernelHandler, MMUFlags};

use alloc::{string::String, vec, vec::Vec};

#[cfg(feature = "board_qemu")]
global_asm!(include_str!("arch/riscv/boot/boot_qemu.asm"));
#[cfg(feature = "board_d1")]
global_asm!(include_str!("arch/riscv/boot/boot_d1.asm"));
#[cfg(target_arch = "riscv64")]
global_asm!(include_str!("arch/riscv/boot/entry64.asm"));

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    logging::init();
    memory::init_heap();

    let config = KernelConfig {
        cmdline: boot_info.cmdline,
        initrd_start: boot_info.initramfs_addr as _,
        initrd_size: boot_info.initramfs_size as _,

        memory_map: &boot_info.memory_map,
        phys_to_virt_offset: boot_info.physical_memory_offset as _,
        graphic_info: boot_info.graphic_info,

        acpi_rsdp: boot_info.acpi2_rsdp_addr,
        smbios: boot_info.smbios_addr,
        ap_fn: secondary_main,
    };
    kernel_hal::primary_init_early(config, &ZcoreKernelHandler);

    let cmdline = kernel_hal::boot::cmdline();
    logging::set_max_level(get_log_level(cmdline));

    memory::init_frame_allocator(&kernel_hal::mem::free_pmem_regions());
    kernel_hal::primary_init();

    let ramfs_data = unsafe {
        core::slice::from_raw_parts_mut(
            (boot_info.initramfs_addr + boot_info.physical_memory_offset) as *mut u8,
            boot_info.initramfs_size as usize,
        )
    };
    main(ramfs_data, cmdline);
}

#[cfg(feature = "zircon")]
fn main(ramfs_data: &[u8], cmdline: &str) -> ! {
    use zircon_loader::{run_userboot, Images};
    let images = Images::<&[u8]> {
        userboot: include_bytes!("../../prebuilt/zircon/x64/userboot.so"),
        vdso: include_bytes!("../../prebuilt/zircon/x64/libzircon.so"),
        zbi: ramfs_data,
    };
    let _proc = run_userboot(&images, cmdline);
    run();
}

#[cfg(target_arch = "riscv64")]
#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    logging::init();
    memory::init_heap();

    println!(
        "zCore rust_main( hartid: {}, device_tree_paddr: {:#x} )",
        hartid, device_tree_paddr
    );
    use arch::consts::*;
    let config = KernelConfig {
        phys_mem_start: PHYS_MEMORY_BASE,
        phys_mem_end: PHYS_MEMORY_END,
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    };
    kernel_hal::primary_init_early(config, &ZcoreKernelHandler);

    let cmdline = kernel_hal::boot::cmdline();
    logging::set_max_level(get_log_level(cmdline));

    memory::init_frame_allocator(&kernel_hal::mem::free_pmem_regions());
    kernel_hal::primary_init();

    // riscv64在之后使用ramfs或virtio, 而x86_64则由bootloader载入文件系统镜像到内存
    main(&mut [], &cmdline);
}

fn secondary_main() -> ! {
    kernel_hal::secondary_init();
    run()
}

#[cfg(feature = "linux")]
fn get_rootproc(cmdline: &str) -> Vec<String> {
    for opt in cmdline.split(':') {
        // parse 'key=value'
        let mut iter = opt.trim().splitn(2, '=');
        let key = iter.next().expect("failed to parse key");
        let value = iter.next().expect("failed to parse value");
        info!("value {}", value);
        if key == "ROOTPROC" {
            let mut iter = value.trim().splitn(2, '?');
            let k1 = iter.next().expect("failed to parse k1");
            let v1 = iter.next().expect("failed to parse v1");
            if v1.is_empty() {
                return vec![k1.into()];
            } else {
                return vec![k1.into(), v1.into()];
            }
        }
    }
    vec!["/bin/busybox".into(), "sh".into()]
}

#[cfg(feature = "linux")]
fn main(ramfs_data: &'static mut [u8], cmdline: &str) -> ! {
    //let args: Vec<String> = vec!["/bin/busybox".into(), "sh".into()];
    let args = get_rootproc(cmdline);
    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin".into()];

    let rootfs = fs::init_filesystem(ramfs_data);
    let _proc = linux_loader::run(args, envs, rootfs);
    info!("linux_loader is complete");

    run();
}

fn run() -> ! {
    loop {
        executor::run_until_idle();
        kernel_hal::interrupt::wait_for_interrupt();
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

struct ZcoreKernelHandler;

impl KernelHandler for ZcoreKernelHandler {
    fn frame_alloc(&self) -> Option<usize> {
        memory::frame_alloc()
    }

    fn frame_alloc_contiguous(&self, frame_count: usize, align_log2: usize) -> Option<usize> {
        memory::frame_alloc_contiguous(frame_count, align_log2)
    }

    fn frame_dealloc(&self, paddr: usize) {
        memory::frame_dealloc(paddr)
    }

    fn handle_page_fault(&self, fault_vaddr: usize, access_flags: MMUFlags) {
        panic!(
            "page fault from kernel mode @ {:#x}({:?})",
            fault_vaddr, access_flags
        );
    }
}
