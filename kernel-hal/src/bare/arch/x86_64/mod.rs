mod acpi_table;
mod apic;
mod trap;

pub mod context;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod serial;
pub mod special;
pub mod timer;
pub mod vm;

use x86_64::registers::control::{Cr4, Cr4Flags};

/// Configuration of HAL.
pub struct HalConfig {
    pub acpi_rsdp: u64,
    pub smbios: u64,
    pub ap_fn: fn() -> !,
}

pub(super) static mut CONFIG: HalConfig = HalConfig {
    acpi_rsdp: 0,
    smbios: 0,
    ap_fn: || unreachable!(),
};

pub fn init(config: HalConfig) {
    apic::init();
    interrupt::init();
    serial::init();
    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
        // store config
        CONFIG = config;

        // start multi-processors
        fn ap_main() {
            info!("processor {} started", cpu::cpu_id());
            unsafe {
                trapframe::init();
            }
            apic::init();
            let ap_fn = unsafe { CONFIG.ap_fn };
            ap_fn()
        }
        fn stack_fn(pid: usize) -> usize {
            // split and reuse the current stack
            unsafe {
                let mut stack: usize;
                asm!("mov {}, rsp", out(reg) stack);
                stack -= 0x4000 * pid;
                stack
            }
        }
        x86_smpboot::start_application_processors(ap_main, stack_fn, crate::mem::phys_to_virt);
    }
}
