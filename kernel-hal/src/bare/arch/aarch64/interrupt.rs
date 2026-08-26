//! Interrupts management.
use crate::HalResult;
use alloc::vec::Vec;
use cortex_a::asm::wfi;

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            intr_on();
            wfi();
            intr_off();
        }

        fn handle_irq(vector: usize) {
            // TODO: timer and other devices with GIC interrupt controller
            // Cached primary-IRQ reference avoids the RwLock + Arc clone on
            // each interrupt that `all_irq().first_unwrap()` would do.
            crate::drivers::primary_irq().handle_irq(vector);
            if vector == 30 {
                debug!("Timer");
            }
        }

        fn intr_off() {
            unsafe {
                core::arch::asm!("msr daifset, #2");
            }
        }

        fn intr_on() {
            unsafe {
                core::arch::asm!("msr daifclr, #2");
            }
        }

        fn intr_get() -> bool {
            use cortex_a::registers::DAIF;
            use tock_registers::interfaces::Readable;
            !DAIF.is_set(DAIF::I)
        }

        fn send_ipi(cpuid: usize, reason: usize) -> HalResult {
            trace!("ipi [{}] => [{}]: {:x}", super::cpu::cpu_id(), cpuid, reason);
            // A GICv2 SGI target list is 8 bits wide, so only CPUs 0..=7 can be
            // addressed. Masking the id (`cpuid & 7`) instead silently aimed
            // CPU 8's shootdown at CPU 0 while the initiator waited for CPU 8's
            // acknowledgement — which could never arrive.
            if cpuid >= 8 {
                warn!("send_ipi: cpu {} is beyond the GICv2 SGI target list", cpuid);
                return Err(crate::HalError);
            }
            // Push reason into per-CPU IPI queue
            let queue = crate::common::ipi::ipi_queue(cpuid);
            let mut delivered = false;
            if let Some(idx) = queue.alloc_entry() {
                *queue.entry_at(idx) = reason;
                delivered = queue.commit_entry(idx);
            }
            if !delivered {
                // Same contract as x86: the receiver cannot learn this entry's
                // payload, so make its next ack a full flush rather than let the
                // precise path skip an invalidation.
                crate::common::ipi::note_ipi_queue_overflow(cpuid);
            }
            // Send GIC SGI #0 to the target CPU (GICv2 GICD_SGIR)
            // GICD_SGIR: [25:24]=TargetListFilter=0b00 (use list), [23:16]=CPUTargetList, [3:0]=SGIINTID
            let gic_base = crate::hal_fn::mem::phys_to_virt(crate::KCONFIG.gic_base);
            const GICD_SGIR: usize = 0x0F00;
            let val: u32 = (1u32 << cpuid) << 16; // SGI 0
            unsafe {
                core::ptr::write_volatile((gic_base + GICD_SGIR) as *mut u32, val);
            }
            Ok(())
        }

        fn ipi_reason() -> Vec<usize> {
            crate::common::ipi::ipi_reason()
        }
    }
}
