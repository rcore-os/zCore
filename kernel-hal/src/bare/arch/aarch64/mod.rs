pub mod config;
pub mod cpu;
pub mod drivers;
pub mod interrupt;
pub mod mem;
pub mod timer;
pub mod trap;
pub mod vm;
pub mod serial;

use alloc::string::String;
use core::ops::Range;
use crate::{mem::phys_to_virt, utils::init_once::InitOnce, PhysAddr};

static CMDLINE: InitOnce<String> = InitOnce::new_with_default(String::new());
static INITRD_REGION: InitOnce<Option<Range<PhysAddr>>> = InitOnce::new_with_default(None);

pub fn cmdline() -> String {
    CMDLINE.clone()
}

pub fn init_ram_disk() -> Option<&'static mut [u8]> {
    INITRD_REGION.as_ref().map(|range| unsafe {
        core::slice::from_raw_parts_mut(phys_to_virt(range.start) as *mut u8, range.len())
    })
}

pub fn primary_init_early() {
    unimplemented!()
}

pub fn primary_init() {
    unimplemented!()
}

pub fn secondary_init() {
    unimplemented!()
}