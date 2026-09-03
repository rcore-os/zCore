mod drivers;
mod trap;

pub mod config;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod timer;
pub mod vm;

#[doc(cfg(target_arch = "x86_64"))]
pub mod special;

hal_fn_impl_default!(crate::hal_fn::console);

use crate::{mem::phys_to_virt, KCONFIG};
use x86_64::registers::control::{Cr4, Cr4Flags};

pub const fn timer_interrupt_vector() -> usize {
    trap::X86_INT_APIC_TIMER
}

pub fn cmdline() -> alloc::string::String {
    KCONFIG.cmdline.into()
}

pub fn init_ram_disk() -> Option<&'static mut [u8]> {
    let start = phys_to_virt(KCONFIG.initrd_start as usize);
    Some(unsafe { core::slice::from_raw_parts_mut(start as *mut u8, KCONFIG.initrd_size as usize) })
}

pub fn primary_init_early() {
    // init serial output first
    drivers::init_early().unwrap();
}

/// Relocate the firmware GDT descriptor into the direct physical mapping.
///
/// Recent rboot versions leave GDTR pointing at the identity-mapped firmware
/// allocation while zCore runs with only the higher-half physical mapping.
/// trapframe copies the current GDT before installing its own, so make that
/// descriptor reachable first.
///
/// # Safety
///
/// Must run after `KCONFIG` and the higher-half physical mapping are ready.
pub unsafe fn prepare_trapframe() {
    #[repr(C, packed)]
    struct DescriptorTablePointer {
        limit: u16,
        base: u64,
    }

    let mut gdtr = DescriptorTablePointer { limit: 0, base: 0 };
    unsafe { core::arch::asm!("sgdt [{}]", in(reg) &mut gdtr) };
    let base = unsafe { core::ptr::addr_of!(gdtr.base).read_unaligned() };
    if base < KCONFIG.phys_to_virt_offset as u64 {
        let relocated = base + KCONFIG.phys_to_virt_offset as u64;
        unsafe { core::ptr::addr_of_mut!(gdtr.base).write_unaligned(relocated) };
        unsafe { core::arch::asm!("lgdt [{}]", in(reg) &gdtr) };
    }
}

pub fn primary_init() {
    drivers::init().unwrap();

    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
        // x86-smpboot probes a fixed APIC ID range, so avoid sending startup
        // IPIs altogether for the normal single-CPU test configuration.
        if option_env!("SMP") != Some("1") {
            let stack_fn = |pid: usize| -> usize {
                // split and reuse the current stack
                let mut stack: usize;
                core::arch::asm!("mov {}, rsp", out(reg) stack);
                stack -= 0x4000 * pid;
                stack
            };
            x86_smpboot::start_application_processors(
                || (crate::KCONFIG.ap_fn)(),
                stack_fn,
                phys_to_virt,
            );
        }
    }
}

pub fn timer_init() {
    timer::init();
}

pub fn secondary_init() {
    zcore_drivers::irq::x86::Apic::init_local_apic_ap();
}
