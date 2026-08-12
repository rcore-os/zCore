use core::arch::global_asm;
use cortex_a::registers::*;
use tock_registers::interfaces::Readable;

mod context;

pub use context::*;

global_asm!(include_str!("switch.S"));
global_asm!(include_str!("executor_entry.S"));

extern "C" {
    pub fn switch(old: *const ContextData, new: *const ContextData);
    pub fn executor_entry();
}

pub(crate) fn cpu_id() -> u8 {
    (MPIDR_EL1.get() & 0xf) as u8
}

pub(crate) fn pg_base_addr() -> usize {
    TTBR0_EL1.get() as usize
}

pub(crate) fn pg_base_register() -> usize {
    TTBR0_EL1.get() as usize
}

pub(crate) fn wait_for_interrupt() {
    let enable = intr_get();
    if !enable {
        intr_on();
    }
    cortex_a::asm::wfi();
    if !enable {
        intr_off();
    }
}

pub(crate) fn intr_on() {
    unsafe {
        core::arch::asm!("msr daifclr, #2");
    }
}

pub(crate) fn intr_off() {
    unsafe {
        core::arch::asm!("msr daifset, #2");
    }
}

pub(crate) fn intr_get() -> bool {
    !DAIF.is_set(DAIF::I)
}
