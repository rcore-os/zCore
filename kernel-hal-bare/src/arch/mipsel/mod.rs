use core::time::Duration;
// use {super::super::*, mips};

pub mod interrupt;
mod io;
mod memory;

pub use interrupt::*;
pub use io::*;
pub use memory::*;

#[allow(dead_code)]
extern "C" {
    fn _root_page_table_buffer();
    fn _root_page_table_ptr();
}

pub unsafe fn set_page_table(vmtoken: usize) {
    use mips::tlb::TLBEntry;
    TLBEntry::clear_all();
    *(_root_page_table_ptr as *mut usize) = vmtoken;
}

pub struct Config {}

pub fn init(_config: Config) {
    intr_init();
}

#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    Duration::from_nanos(0)
}

#[export_name = "hal_apic_local_id"]
pub fn apic_local_id() -> u8 {
    // unimplemented!()
    0
}
