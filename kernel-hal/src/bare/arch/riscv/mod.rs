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

pub fn cmdline() -> alloc::string::String {
    // TODO: get bootargs from device tree.
    "LOG=warn:TERM=xterm-256color:console.shell=true:virtcon.disable=true".into()
}

pub fn init_ram_disk() -> Option<&'static mut [u8]> {
    // TODO: get initrd start & end from device tree.
    None
}

pub fn primary_init_early() {}

pub fn primary_init() {
    vm::init();
    drivers::init().unwrap();
    timer::init();
}

pub fn secondary_init() {}
