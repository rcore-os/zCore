//! Interrupts management.
use crate::{HalError, HalResult};
use alloc::vec::Vec;
use riscv::{asm, register::sstatus};

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            let enable = sstatus::read().sie();
            if !enable {
                unsafe { sstatus::set_sie() };
            }
            unsafe { asm::wfi(); }
            if !enable {
                unsafe { sstatus::clear_sie() };
            }
        }

        fn handle_irq(cause: usize) {
            trace!("Handle irq cause: {}", cause);
            let irq = crate::drivers::all_irq()
                .find(alloc::format!("riscv-intc-cpu{}", crate::cpu::cpu_id()).as_str())
                .expect("IRQ device 'riscv-intc' not initialized!");
            irq.handle_irq(cause)
        }

        fn intr_on() {
            unsafe { sstatus::set_sie() };
        }

        fn intr_off() {
            unsafe { sstatus::clear_sie() };
        }

        fn intr_get() -> bool {
            sstatus::read().sie()
        }

        #[allow(deprecated)]
        fn send_ipi(cpuid: usize, reason: usize) -> HalResult {
            trace!("ipi [{}] => [{}]", super::cpu::cpu_id(), cpuid);
            let queue = crate::common::ipi::ipi_queue(cpuid);
            let idx = queue.alloc_entry();
            if let Some(idx) = idx {
                let entry = queue.entry_at(idx);
                *entry = reason;
                queue.commit_entry(idx);
                let mask:usize = 1 << cpuid;
                sbi_rt::legacy::send_ipi(&mask as *const usize as usize);
                return Ok(());
            }
            Err(HalError)
        }

        fn ipi_reason() -> Vec<usize> {
            crate::common::ipi::ipi_reason()
        }
    }
}
