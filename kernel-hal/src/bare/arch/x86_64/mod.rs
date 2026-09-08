mod drivers;
mod smp;
mod tlb;
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

/// Install the same initial GDT on the BSP and every AP.
///
/// trapframe appends descriptors to the current GDT and shares its user segment
/// selectors across CPUs. Firmware and AP trampoline GDTs have different sizes,
/// so normalize them before trapframe computes those selectors.
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

    // Set the accessed bits: this table is in read-only kernel memory.
    static GDT: [u64; 3] = [0, 0x0020_9b00_0000_0000, 0x0000_9300_0000_0000];
    let gdtr = DescriptorTablePointer {
        limit: (core::mem::size_of_val(&GDT) - 1) as u16,
        base: GDT.as_ptr() as u64,
    };
    unsafe {
        core::arch::asm!(
            "lgdt [{gdtr}]",
            "push 8",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov ax, 16",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            gdtr = in(reg) &gdtr,
            out("rax") _,
        );
    }
}

pub fn primary_init() {
    tlb::init();
    drivers::init().unwrap();

    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
    }
    smp::start();
}

pub fn timer_init() {
    timer::init();
}

pub fn secondary_init() {
    tlb::init();
    zcore_drivers::irq::x86::Apic::init_local_apic_ap();
    drivers::init_local_timer();
}
