pub mod config;
pub mod cpu;
pub mod drivers;
pub mod interrupt;
pub mod mem;
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
    use cortex_a::{asm::barrier, registers::CPACR_EL1};
    use tock_registers::interfaces::{Readable, Writeable};

    // User contexts preserve the architectural FP/SIMD state, so permit both
    // EL0 and EL1 to execute those instructions instead of trapping on first use.
    CPACR_EL1.set(CPACR_EL1.get() | (0b11 << 20));
    unsafe { barrier::isb(barrier::SY) };
    CMDLINE.init_once_by(KCONFIG.cmdline.to_string());
    drivers::init_early();
}

pub fn primary_init() {
    vm::init();
    drivers::init();
}

pub fn secondary_init() {
    unimplemented!()
}

pub const fn timer_interrupt_vector() -> usize {
    30
}

pub fn timer_init() {
    timer::init();
}
