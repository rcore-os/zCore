mod drivers;
mod trap;

pub mod config;
pub mod context;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod special;
pub mod timer;
pub mod vm;

hal_fn_impl_default!(crate::hal_fn::console);

use x86_64::registers::control::{Cr4, Cr4Flags};

pub fn init() {
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
            || {
                init_ap();
                (crate::KCONFIG.ap_fn)();
            },
            stack_fn,
            crate::mem::phys_to_virt,
        );
    }
}

pub fn init_ap() {
    info!("processor {} started", cpu::cpu_id());
    unsafe { trapframe::init() };
    zcore_drivers::irq::x86::Apic::init_local_apic_ap();
}
