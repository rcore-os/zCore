//! Bootstrap and initialization.

use crate::{KernelConfig, KernelHandler, KCONFIG, KHANDLER};

hal_fn_impl! {
    impl mod crate::hal_fn::boot {
        fn cmdline() -> alloc::string::String {
            super::arch::cmdline()
        }

        fn init_ram_disk() -> Option<&'static mut [u8]> {
            super::arch::init_ram_disk()
        }

        fn primary_init_early(cfg: KernelConfig, handler: &'static impl KernelHandler) {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            lock::set_phys_virt_offset(cfg.phys_to_virt_offset as u64);
            KCONFIG.init_once_by(cfg);
            KHANDLER.init_once_by(handler);
            crate::klog_info!("Eclipse: primary CPU {} init early", crate::cpu::cpu_id());
            super::arch::primary_init_early();
        }

        fn primary_init() {
            crate::klog_info!("Eclipse: primary CPU {} init", crate::cpu::cpu_id());
            unsafe { trapframe::init() };
            crate::vm::pin_kernel_vmtoken();
            // Bind this CPU to its PercpuBlock (sets the GS fast-path on x86_64).
            super::percpu::register();
            // Let the scheduler kick halted CPUs on cross-CPU wakes instead of
            // waiting for their next periodic tick (up to 4 ms of latency per
            // pipe write / IO completion / process exit otherwise).
            executor::set_resched_ipi_sender(|cpu| {
                crate::interrupt::send_wake_ipi(cpu);
            });
            super::arch::primary_init();
        }

        fn secondary_init() {
            #[cfg(target_arch = "x86_64")]
            {
                let logical = super::arch::ap_trampoline_logical_id();
                // We have now latched our logical id out of the shared trampoline
                // slot — release the BSP to reuse the slots for the next AP. Do
                // this *before* any slow per-CPU init so a busy-waiting BSP can't
                // outrun us and clobber the slot mid-flight (see `smp.rs`).
                super::arch::ap_signal_slot_consumed();
                lock::with_ap_boot_logical(logical, || unsafe {
                    trapframe::init_ap();
                });
                unsafe {
                    trapframe::write_logical_cpu_id(logical);
                }
                lock::set_logical_cpu_id(lock::hardware_apic_id(), logical);
            }
            #[cfg(not(target_arch = "x86_64"))]
            unsafe {
                trapframe::init();
            }
            super::percpu::register();
            super::arch::secondary_init();
        }
    }
}
