mod drivers;
mod trap;

pub mod config;
pub mod context;
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

pub fn primary_init() {
    drivers::init().unwrap();

    let stack_fn = |pid: usize| -> usize {
        // split and reuse the current stack
        let mut stack: usize;
        unsafe { asm!("mov {}, rsp", out(reg) stack) };
        stack -= 0x4000 * pid;
        stack
    };
    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
        // start multi-processors
        x86_smpboot::start_application_processors(
            || (crate::KCONFIG.ap_fn)(),
            stack_fn,
            phys_to_virt,
        );
    }
}

pub fn secondary_init() {
    zcore_drivers::irq::x86::Apic::init_local_apic_ap();
}
