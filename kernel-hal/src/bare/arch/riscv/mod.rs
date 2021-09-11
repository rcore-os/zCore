#![allow(dead_code)]

#[macro_use]
pub mod serial;

mod consts;
mod plic;
mod sbi;
mod trap;
mod uart;

pub mod config;
pub mod context;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod special;
pub mod timer;
pub mod vm;

pub fn init() {
    vm::remap_the_kernel().unwrap();
    interrupt::init();
    timer::init();

    unsafe { asm!("ebreak") };

    #[cfg(feature = "board_qemu")]
    {
        // TODO
        // sbi_println!("Setup virtio @devicetree {:#x}", cfg.dtb);
        // drivers::virtio::device_tree::init(cfg.dtb);
    }
}
