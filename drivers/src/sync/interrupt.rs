use core::cell::{RefCell, RefMut};

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "none", any(target_arch = "riscv32", target_arch = "riscv64")))] {
        mod interrupts {
            use riscv::register::sstatus;
            pub(crate) fn cpu_id() -> u8 {
                let mut cpu_id;
                unsafe {
                    core::arch::asm!("mv {0}, tp", out(reg) cpu_id);
                }
                cpu_id
            }
            pub(crate) fn intr_on() {
                unsafe { sstatus::set_sie() };
            }
            pub(crate) fn intr_off() {
                unsafe { sstatus::clear_sie() };
            }
            pub(crate) fn intr_get() -> bool {
                sstatus::read().sie()
            }
        }
    } else if #[cfg(all(target_os = "none", any(target_arch = "x86", target_arch = "x86_64")))] {
        mod interrupts {
            use x86_64::instructions::interrupts;
            pub(crate) fn cpu_id() -> u8 {
                raw_cpuid::CpuId::new()
                    .get_feature_info()
                    .unwrap()
                    .initial_local_apic_id() as u8
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
        }
    } else if #[cfg(all(target_os = "none", target_arch = "aarch64"))] {
        mod interrupts {
            pub(crate) fn cpu_id() -> u8 {
                use cortex_a::registers::MPIDR_EL1;
                use tock_registers::interfaces::Readable;
                (MPIDR_EL1.get() & 0xf) as u8
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
                use cortex_a::registers::DAIF;
                use tock_registers::interfaces::Readable;
                !DAIF.is_set(DAIF::I)
            }
        }
    } else {
        mod interrupts {
            pub(crate) fn cpu_id() -> u8 {
                unimplemented!();
            }
            pub(crate) fn intr_on() { unimplemented!(); }
            pub(crate) fn intr_off() { unimplemented!(); }
            pub(crate) fn intr_get() -> bool {
                unimplemented!();
            }
        }
    }
}

use interrupts::*;

#[derive(Debug, Default, Clone, Copy)]
#[repr(align(64))]
pub struct Cpu {
    pub noff: i32,              // Depth of push_off() nesting.
    pub interrupt_enable: bool, // Were interrupts enabled before push_off()?
}

impl Cpu {
    const fn new() -> Self {
        Self {
            noff: 0,
            interrupt_enable: false,
        }
    }
}

pub struct SafeRefCell<T>(RefCell<T>);

// #Safety: Only the corresponding cpu will access it.
unsafe impl<Cpu> Sync for SafeRefCell<Cpu> {}

impl<T> SafeRefCell<T> {
    const fn new(t: T) -> Self {
        Self(RefCell::new(t))
    }
}

// Avoid hard code
#[allow(clippy::declare_interior_mutable_const)]
const DEFAULT_CPU: SafeRefCell<Cpu> = SafeRefCell::new(Cpu::new());

const MAX_CORE_NUM: usize = 16;

static CPUS: [SafeRefCell<Cpu>; MAX_CORE_NUM] = [DEFAULT_CPU; MAX_CORE_NUM];

pub fn mycpu() -> RefMut<'static, Cpu> {
    CPUS[cpu_id() as usize].0.borrow_mut()
}

// push_off/pop_off are like intr_off()/intr_on() except that they are matched:
// it takes two pop_off()s to undo two push_off()s.  Also, if interrupts
// are initially off, then push_off, pop_off leaves them off.
pub(crate) fn push_off() {
    let old = intr_get();
    intr_off();
    let mut cpu = mycpu();
    if cpu.noff == 0 {
        cpu.interrupt_enable = old;
    }
    cpu.noff += 1;
}

pub(crate) fn pop_off() {
    let mut cpu = mycpu();
    if intr_get() || cpu.noff < 1 {
        panic!("pop_off");
    }
    cpu.noff -= 1;
    let should_enable = cpu.noff == 0 && cpu.interrupt_enable;
    drop(cpu);
    // NOTICE: intr_on() may lead to an immediate inerrupt, so we *MUST* drop(cpu) in advance.
    if should_enable {
        intr_on();
    }
}
