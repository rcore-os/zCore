//! Interrupts management.

use core::ops::Range;

use crate::drivers::all_irq;
use crate::drivers::prelude::{IrqHandler, IrqPolarity, IrqTriggerMode};
use crate::HalResult;
use alloc::vec::Vec;
use x86_64::instructions::interrupts;

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            // Park this CPU until the next interrupt. `cpu_idle` uses
            // MONITOR/MWAIT (C1E) when the hardware supports it — cooler than a
            // bare `hlt` (C1) — and falls back to `sti; hlt` otherwise, while
            // preserving the caller's interrupt-enable state. See `super::power`.
            super::power::cpu_idle();
        }

        fn is_valid_irq(gsi: usize) -> bool {
            all_irq().first_unwrap().is_valid_irq(gsi)
        }

        fn intr_on() {
            interrupts::enable();
        }

        fn intr_off() {
            interrupts::disable();
        }

        fn intr_get() -> bool {
            interrupts::are_enabled()
        }

        fn mask_irq(gsi: usize) -> HalResult {
            Ok(all_irq().first_unwrap().mask(gsi)?)
        }

        fn unmask_irq(gsi: usize) -> HalResult {
            Ok(all_irq().first_unwrap().unmask(gsi)?)
        }

        fn configure_irq(gsi: usize, tm: IrqTriggerMode, pol: IrqPolarity) -> HalResult {
            Ok(all_irq().first_unwrap().configure(gsi, tm, pol)?)
        }

        fn register_irq_handler(gsi: usize, handler: IrqHandler) -> HalResult {
            Ok(all_irq().first_unwrap().register_handler(gsi, handler)?)
        }

        fn unregister_irq_handler(gsi: usize) -> HalResult {
            Ok(all_irq().first_unwrap().unregister(gsi)?)
        }

        fn handle_irq(vector: usize) {
            crate::kstats::note_irq(vector);
            // Cached primary-IRQ reference: every interrupt would otherwise
            // pay an RwLock read + Arc clone via `all_irq().first_unwrap()`.
            crate::drivers::primary_irq().handle_irq(vector as usize);
        }

        fn msi_alloc_block(requested_irqs: usize) -> HalResult<Range<usize>> {
            Ok(all_irq().first_unwrap().msi_alloc_block(requested_irqs)?)
        }

        fn msi_free_block(block: Range<usize>) -> HalResult {
            Ok(all_irq().first_unwrap().msi_free_block(block)?)
        }

        fn msi_register_handler(
            block: Range<usize>,
            msi_id: usize,
            handler: IrqHandler,
        ) -> HalResult {
            Ok(all_irq().first_unwrap().msi_register_handler(block, msi_id, handler)?)
        }

        fn send_ipi(cpuid: usize, reason: usize) -> HalResult {
            // `cpuid` is a dense logical CPU id: it indexes the per-CPU IPI queue
            // directly, but the LAPIC needs the *hardware* APIC ID, so translate
            // through the topology map before delivering the interrupt.
            trace!("ipi [{}] => [{}]: {:x}", super::cpu::cpu_id(), cpuid, reason);
            let Some(apic_id) = super::smp::logical_to_apic(cpuid) else {
                warn!(
                    "send_ipi: logical cpu {} has no APIC mapping — dropped",
                    cpuid
                );
                return Ok(());
            };
            let queue = crate::common::ipi::ipi_queue(cpuid);
            let mut delivered = false;
            if let Some(idx) = queue.alloc_entry() {
                *queue.entry_at(idx) = reason;
                delivered = queue.commit_entry(idx);
            }
            if !delivered {
                // Queue full or commit lost the publish race: the receiver
                // cannot learn this entry's payload, so force it to treat the
                // next ack as a full flush. Without this, the precise
                // (per-page) ack path would silently skip an invalidation.
                crate::common::ipi::note_ipi_queue_overflow(cpuid);
            }
            // X86_INT_LOCAL_APIC_BASE + 3 = 0xf3, our IPI vector
            const IPI_VECTOR: u8 = 0xf3;
            zcore_drivers::irq::x86::Apic::send_ipi_to(IPI_VECTOR, apic_id);
            Ok(())
        }

        fn send_wake_ipi(cpuid: usize) {
            // Reschedule kick for a halted executor: deliberately NO queue
            // entry — the 0xf3 handler treats an empty drain as a pure wake
            // (no TLB flush, no shootdown-sequence bump), so this is as cheap
            // as an interrupt can be on the receiving side.
            const IPI_VECTOR: u8 = 0xf3;
            if zcore_drivers::irq::x86::Apic::local_apic_ready() {
                if let Some(apic_id) = super::smp::logical_to_apic(cpuid) {
                    zcore_drivers::irq::x86::Apic::send_ipi_to(IPI_VECTOR, apic_id);
                }
            }
        }

        fn ipi_reason() -> Vec<usize> {
            crate::common::ipi::ipi_reason()
        }
    }
}
