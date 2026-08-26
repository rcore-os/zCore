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
            // Per-hart cache: the previous code did a String allocation
            // (`format!("riscv-intc-cpu{}", hart)`) plus a linear scan over
            // the device list on every interrupt. Once the per-hart intc is
            // located, store it so subsequent IRQs only pay an indexed load
            // + a single Acquire on `Once`.
            use alloc::sync::Arc;
            use zcore_drivers::scheme::IrqScheme;
            static IRQ_PER_HART: [spin::Once<Arc<dyn IrqScheme>>; crate::config::MAX_CORE_NUM] =
                [const { spin::Once::new() }; crate::config::MAX_CORE_NUM];
            let hart = super::cpu::raw_hart_id();
            let arc = IRQ_PER_HART[hart].call_once(|| {
                crate::drivers::all_irq()
                    .find(alloc::format!("riscv-intc-cpu{}", hart).as_str())
                    .expect("IRQ device 'riscv-intc' not initialized!")
            });
            arc.handle_irq(cause)
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
                // `cpuid` is a dense logical id (queue index); SBI needs a hart mask.
                let mask: usize = 1 << super::cpu::logical_to_hart(cpuid);
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
