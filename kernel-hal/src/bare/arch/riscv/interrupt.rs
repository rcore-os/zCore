//! Interrupts management.

use crate::{config::MAX_CORE_NUM, HalError, HalResult};
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
            crate::drivers::all_irq().first_unwrap().handle_irq(cause)
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
            let queue = crate::ipi::ipi_queue(cpuid);
            let idx = queue.apply_entry();
            if let Some(idx) = idx {
                let entry = queue.entry_at(idx);
                *entry = reason;
                queue.submit_entry(idx);
                assert!(MAX_CORE_NUM <= 64);
                let mask:usize = 1 << cpuid;
                sbi_rt::legacy::send_ipi(&mask as *const usize as usize);
                return Ok(());
            }
            Err(HalError)
        }

        fn ipi_reason() -> Vec<usize> {
            crate::ipi::ipi_reason()
        }
    }
}
