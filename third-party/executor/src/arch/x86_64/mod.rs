use core::arch::{asm, global_asm};

mod context;

pub use context::*;

global_asm!(include_str!("switch.S"));
global_asm!(include_str!("executor_entry.S"));

extern "C" {
    pub fn switch(old: *const ContextData, new: *const ContextData);
    pub fn executor_entry();
}

pub(crate) fn cpu_id() -> u8 {
    raw_cpuid::CpuId::new()
        .get_feature_info()
        .unwrap()
        .initial_local_apic_id() as u8
}

// pub(crate) fn pg_base_addr() -> usize {
//     x86_64::registers::control::Cr3::read()
//         .0
//         .start_address()
//         .as_u64() as _
// }

pub(crate) fn pg_base_register() -> usize {
    let mut cr3;
    unsafe {
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    cr3
}

use x86_64::instructions::interrupts;

pub(crate) fn wait_for_interrupt() {
    let enable = interrupts::are_enabled();
    interrupts::enable_and_hlt();
    if !enable {
        interrupts::disable();
    }
}

pub(crate) fn intr_on() {
    interrupts::enable();
}

pub(crate) fn intr_off() {
    interrupts::disable();
}

pub(crate) fn intr_get() -> bool {
    interrupts::are_enabled()
}
