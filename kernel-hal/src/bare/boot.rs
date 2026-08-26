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
            // Unmapped guard pages under coroutine stacks — must run after the
            // kernel page tables are pinned so install can punch 4K holes in
            // the BSS heap mapping (see `stack_guard`). Must also run *before*
            // any `Executor::new`: warm the lazy GLOBAL_RUNTIME here so the
            // first strong stacks get hard guards (not soft-only leftovers).
            super::stack_guard::init();
            if !executor::stack_guard_hooks_registered() {
                panic!("stack_guard::init failed to register executor hooks");
            }
            executor::warm_runtimes();
            // Say out loud whether the coroutine stacks actually got unmapped
            // guard bands. Without them an overflow silently overwrites
            // neighbouring heap instead of faulting, and the machine dies later
            // and elsewhere — indirect calls to `rip=0x0`/`0x3`, `Arc` vtables
            // replaced by small integers — with nothing linking the wreckage
            // back to the stack that caused it. That failure is expensive
            // enough to diagnose that its precondition belongs in the boot log.
            let (installed, refused) = super::stack_guard::stats();
            let (hard, soft) = executor::hard_guard_executor_counts();
            crate::klog_info!(
                "stack_guard: {} guard band(s) installed, {} refused; \
                 executors hard={} soft={}",
                installed,
                refused,
                hard,
                soft
            );
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
                // Provisional: this AP's LAPIC is still in xAPIC mode (INIT
                // leaves it there, whatever mode the BSP runs in), so this is
                // the 8-bit id. `secondary_init` re-registers the authoritative
                // one once the AP has switched its own LAPIC to x2APIC.
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
