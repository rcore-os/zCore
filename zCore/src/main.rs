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

#[cfg(target_arch = "riscv64")]
use kernel_hal::{vm::remap_the_kernel, BootInfo, GraphicInfo};

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

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

    kernel_hal::init(kernel_hal::HalConfig {
        acpi_rsdp: boot_info.acpi2_rsdp_addr,
        smbios: boot_info.smbios_addr,
        ap_fn: run,
    });

    #[cfg(feature = "graphic")]
    {
        let (width, height) = boot_info.graphic_info.mode.resolution();
        let fb_addr = boot_info.graphic_info.fb_addr as usize;
        let fb_size = boot_info.graphic_info.fb_size as usize;
        kernel_hal::dev::fb::init(width as u32, height as u32, fb_addr, fb_size);
    }

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
    let device_tree_vaddr = device_tree_paddr + arch::consts::PHYSICAL_MEMORY_OFFSET;

    let boot_info = BootInfo {
        memory_map: Vec::new(),
        physical_memory_offset: 0,
        graphic_info: GraphicInfo {
            mode: 0,
            fb_addr: 0,
            fb_size: 0,
        },
        hartid: hartid as u64,
        dtb_addr: device_tree_paddr as u64,
        initramfs_addr: 0,
        initramfs_size: 0,
        cmdline: "LOG=warn:TERM=xterm-256color:console.shell=true:virtcon.disable=true",
    };

    logging::init(get_log_level(boot_info.cmdline));
    memory::init_heap();
    memory::init_frame_allocator(&boot_info);
    remap_the_kernel(device_tree_vaddr);

    info!("{:#x?}", boot_info);

    kernel_hal::init(kernel_hal::HalConfig {
        mconfig: 0,
        dtb: device_tree_vaddr,
    });

    let cmdline_dt = "";// FIXME: CMDLINE.read();
    let mut cmdline = boot_info.cmdline.to_string();

    if !cmdline_dt.is_empty() {
        cmdline = format!("{}:{}", boot_info.cmdline, cmdline_dt);
    };
    warn!("cmdline: {:?}", cmdline);

    #[cfg(feature = "graphic")]
    {
        let gpu = GPU_DRIVERS
            .read()
            .iter()
            .next()
            .expect("Gpu device not found")
            .clone();
        let (width, height) = gpu.resolution();
        let (fb_vaddr, fb_size) = gpu.setup_framebuffer();
        kernel_hal::deb::fb::init(width, height, fb_vaddr, fb_size);
    }

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
            if v1 == "" {
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

    kernel_hal::serial::serial_set_callback(Box::new({
        move || {
            let mut buffer = [0; 255];
            let len = kernel_hal::serial::serial_read(&mut buffer);
            for c in &buffer[..len] {
                STDIN.push((*c).into());
                // kernel_hal::serial::serial_write(alloc::format!("{}", *c as char).as_str());
            }
            false
        }
    }));

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
        #[cfg(target_arch = "x86_64")]
        {
            x86_64::instructions::interrupts::enable_and_hlt();
            x86_64::instructions::interrupts::disable();
        }
        #[cfg(target_arch = "riscv64")]
        kernel_hal::riscv::wait_for_interrupt();
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
