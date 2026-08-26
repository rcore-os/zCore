pub mod config;
pub mod cpu;
pub mod drivers;
pub mod interrupt;
pub mod mem;
pub mod smp;
pub mod timer;
pub mod trap;
pub mod vm;

use crate::KCONFIG;
use crate::{mem::phys_to_virt, utils::init_once::InitOnce, PhysAddr};
use alloc::string::{String, ToString};
use core::ops::Range;

hal_fn_impl_default!(crate::hal_fn::console);

static INITRD_REGION: InitOnce<Option<Range<PhysAddr>>> = InitOnce::new_with_default(None);
static CMDLINE: InitOnce<String> = InitOnce::new_with_default(String::new());

pub fn cmdline() -> String {
    CMDLINE.clone()
}

pub fn init_ram_disk() -> Option<&'static mut [u8]> {
    INITRD_REGION.as_ref().map(|range| unsafe {
        core::slice::from_raw_parts_mut(phys_to_virt(range.start) as *mut u8, range.len())
    })
}

pub fn primary_init_early() {
    CMDLINE.init_once_by(KCONFIG.cmdline.to_string());
    drivers::init_early();
}

pub fn primary_init() {
    vm::init();
    drivers::init();
    // Bring up secondary cores now that the kernel page table and GIC are ready.
    smp::start_secondary_cores();
}

pub fn secondary_init() {
    // Enable this core's GIC CPU interface so it can receive SGIs/PPIs.
    unsafe {
        let gicc = phys_to_virt(KCONFIG.gic_base + 0x1_0000);
        core::ptr::write_volatile(gicc as *mut u32, 1); // GICC_CTLR = 1 (enable)
        core::ptr::write_volatile((gicc + 0x4) as *mut u32, 0xff); // GICC_PMR = 0xff
    }
    // Re-enable interrupts on this AP; GIC SGI are always-on.
    interrupt::intr_on();
    smp::ap_signal_online();
}

pub const fn timer_interrupt_vector() -> usize {
    30
}

pub fn timer_init() {
    timer::init();
}
