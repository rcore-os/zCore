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

use alloc::{boxed::Box, string::String, vec, vec::Vec};

use crate::arch::consts::*;

#[cfg(feature = "board_qemu")]
global_asm!(include_str!("arch/riscv/boot/boot_qemu.asm"));
#[cfg(feature = "board_d1")]
global_asm!(include_str!("arch/riscv/boot/boot_d1.asm"));
#[cfg(target_arch = "riscv64")]
global_asm!(include_str!("arch/riscv/boot/entry64.asm"));

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn _start(boot_info: &BootInfo) -> ! {
    logging::init(get_log_level(boot_info.cmdline));
    memory::init_heap();
    memory::init_frame_allocator(boot_info);

    trace!("{:#x?}", boot_info);

    use kernel_hal::drivers::prelude::{ColorFormat, DisplayInfo};
    let graphic_info = &boot_info.graphic_info;
    let (width, height) = graphic_info.mode.resolution();
    let display_info = DisplayInfo {
        width: width as _,
        height: height as _,
        format: ColorFormat::ARGB8888, // uefi::proto::console::gop::PixelFormat::Bgr
        fb_base_vaddr: graphic_info.fb_addr as usize + PHYSICAL_MEMORY_OFFSET,
        fb_size: graphic_info.fb_size as usize,
    };

    let config = KernelConfig {
        kernel_offset: KERNEL_OFFSET,
        phys_mem_start: PHYSICAL_MEMORY_OFFSET,
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        display_info,
        acpi_rsdp: boot_info.acpi2_rsdp_addr,
        smbios: boot_info.smbios_addr,
        ap_fn: run,
    };
    info!("{:#x?}", config);
    kernel_hal::init(config, &ZcoreKernelHandler);

    let ramfs_data = unsafe {
        core::slice::from_raw_parts_mut(
            (boot_info.initramfs_addr + boot_info.physical_memory_offset) as *mut u8,
            boot_info.initramfs_size as usize,
        )
    };
    main(ramfs_data, boot_info.cmdline);
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
    println!(
        "zCore rust_main( hartid: {}, device_tree_paddr: {:#x} )",
        hartid, device_tree_paddr
    );
    let cmdline = "LOG=warn:TERM=xterm-256color:console.shell=true:virtcon.disable=true";
    let config = KernelConfig {
        kernel_offset: KERNEL_OFFSET,
        phys_mem_start: MEMORY_OFFSET,
        phys_mem_end: MEMORY_END,
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    };

    logging::init(get_log_level(cmdline));
    memory::init_heap();
    memory::init_frame_allocator();

    info!("{:#x?}", config);
    kernel_hal::init(config, &ZcoreKernelHandler);

    let cmdline_dt = ""; // FIXME: CMDLINE.read();
    let cmdline = if !cmdline_dt.is_empty() {
        alloc::format!("{}:{}", cmdline, cmdline_dt)
    } else {
        use alloc::string::ToString;
        cmdline.to_string()
    };
    warn!("cmdline: {:?}", cmdline);

    // riscv64在之后使用ramfs或virtio, 而x86_64则由bootloader载入文件系统镜像到内存
    main(&mut [], &cmdline);
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
    use linux_object::fs::STDIN;

    if let Some(uart) = kernel_hal::drivers::all_uart().first() {
        uart.clone().subscribe(
            Box::new(move |_| {
                while let Some(c) = uart.try_recv().unwrap_or(None) {
                    STDIN.push(c as char);
                }
            }),
            false,
        );
    }

    //let args: Vec<String> = vec!["/bin/busybox".into(), "sh".into()];
    let args: Vec<String> = get_rootproc(cmdline);
    let envs: Vec<String> = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin".into()];

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
