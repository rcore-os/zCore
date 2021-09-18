#![allow(dead_code)]

mod consts;
mod drivers;
mod plic;
mod sbi;
mod trap;

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
    drivers::init();
    interrupt::init();
    timer::init();

    #[cfg(feature = "board_qemu")]
    {
        // TODO
        // sbi_println!("Setup virtio @devicetree {:#x}", cfg.dtb);
        // drivers::virtio::device_tree::init(cfg.dtb);
    }
}
