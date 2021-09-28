#![allow(dead_code)]

mod consts;
mod drivers;
mod sbi;
mod trap;

pub mod config;
pub mod context;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod timer;
pub mod vm;

pub fn init() {
    vm::remap_the_kernel().unwrap();
    drivers::init().unwrap();
    timer::init();
}
