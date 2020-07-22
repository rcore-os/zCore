use core::time::Duration;
// use {super::super::*, mips};

mod interrupt;
mod io;

pub use interrupt::*;
pub use io::*;

pub unsafe fn set_page_table(_vmtoken: usize) {
    unimplemented!();
}

pub struct Config {}

pub fn init(_config: Config) {
    intr_init();
}

#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    unimplemented!();
}

#[export_name = "hal_apic_local_id"]
pub fn apic_local_id() -> u8 {
    unimplemented!()
}
