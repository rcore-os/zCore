mod acpi_table;
mod apic;
mod trap;

pub mod config;
pub mod context;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod serial;
pub mod special;
pub mod timer;
pub mod vm;

use x86_64::registers::control::{Cr4, Cr4Flags};

pub fn init(cfg: config::KernelConfig) {
    crate::CONFIG.call_once(|| cfg);
    apic::init();
    interrupt::init();
    serial::init();

    fn ap_main() {
        info!("processor {} started", cpu::cpu_id());
        unsafe { trapframe::init() };
        apic::init();
        let ap_fn = crate::CONFIG.get().unwrap().ap_fn;
        ap_fn();
    }

    fn stack_fn(pid: usize) -> usize {
        // split and reuse the current stack
        let mut stack: usize;
        unsafe { asm!("mov {}, rsp", out(reg) stack) };
        stack -= 0x4000 * pid;
        stack
    }

    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
        // start multi-processors
        x86_smpboot::start_application_processors(ap_main, stack_fn, crate::mem::phys_to_virt);
    }
}
