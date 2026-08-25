use alloc::{boxed::Box, string::String, sync::Arc, vec::Vec};
use core::{any::Any, future::Future, ops::Range, time::Duration};

use crate::drivers::prelude::{IrqHandler, IrqPolarity, IrqTriggerMode};
use crate::{common, HalResult, KernelConfig, KernelHandler, PhysAddr, VirtAddr};

hal_fn_def! {
    /// Bootstrap and initialization.
    pub mod boot {
        /// The kernel command line.
        ///
        /// TODO: use `&'a str` as return type.
        pub fn cmdline() -> String { String::new() }

        /// Returns the slice of the initial RAM disk, or `None` if not exist.
        pub fn init_ram_disk() -> Option<&'static mut [u8]> {
            None
        }

        /// Initialize the primary CPU at an early stage (before the physical frame allocator).
        pub fn primary_init_early(cfg: KernelConfig, handler: &'static impl KernelHandler) {}

        /// The main part of the primary CPU initialization.
        pub fn primary_init();

        /// Initialize the secondary CPUs.
        pub fn secondary_init() {}
    }

    /// CPU information.
    pub mod cpu {
        /// Current CPU ID.
        pub fn cpu_id() -> u8 { 0 }

        /// Current CPU frequency in MHz.
        pub fn cpu_frequency() -> u16 { 3000 }

        /// Get the CPU brand/model name.
        pub fn cpu_brand() -> String { String::new() }

        /// Get the number of online CPU cores.
        pub fn cpu_count() -> u8 { 1 }

        /// This CPU's temperature in milli-degrees Celsius, read from the digital
        /// thermal sensor, or `None` when the hardware doesn't expose it.
        pub fn cpu_temperature_mc() -> Option<i32> { None }

        /// Adaptive P-state governor summary: `(throttled core count, cpu0's
        /// current P-state ceiling, cpu0's base ceiling)`, or `None` when no OS
        /// P-state control is active (non-HWP/CPPC parts, or under a hypervisor).
        pub fn pstate_governor_summary() -> Option<(u32, u8, u8)> { None }

        /// Shutdown/reboot the machine.
        pub fn reset() -> !;
    }

    /// Physical memory operations.
    pub mod mem: common::mem {
        /// Convert physical address to virtual address.
        pub fn phys_to_virt(paddr: PhysAddr) -> VirtAddr;

        /// Convert virtual address to physical address.
        pub fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr;

        /// Returns all free physical memory regions.
        pub fn free_pmem_regions() -> Vec<Range<PhysAddr>>;

        /// Read physical memory from `paddr` to `buf`.
        pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]);

        /// Write physical memory to `paddr` from `buf`.
        pub fn pmem_write(paddr: PhysAddr, buf: &[u8]);

        /// Zero physical memory at `[paddr, paddr + len)`.
        pub fn pmem_zero(paddr: PhysAddr, len: usize);

        /// Copy content of physical memory `src` to `dst` with `len` bytes.
        pub fn pmem_copy(dst: PhysAddr, src: PhysAddr, len: usize);

        /// Flush the physical frame.
        pub fn frame_flush(target: PhysAddr);

        /// Get memory usage: (used_bytes, total_bytes)
        pub fn memory_usage() -> (usize, usize) {
            (0, 0)
        }
    }

    /// Virtual memory operations.
    pub mod vm: common::vm {
        /// Read the current VM token, which is the page table root address on
        /// various architectures. (e.g. CR3, SATP, ...)
        pub fn current_vmtoken() -> PhysAddr;

        /// Activate the page table associated with the `vmtoken` by writing the
        /// page table root address.
        pub fn activate_paging(vmtoken: PhysAddr);

        /// Record the kernel page table root once at boot (BSP CR3 / kernel PT).
        pub fn pin_kernel_vmtoken() {}

        /// The kernel's own page-table root, or 0 while unknown. Used to keep
        /// TLB-shootdown target filtering away from kernel-table flushes.
        pub fn kernel_vmtoken() -> PhysAddr {
            0
        }

        /// Restore the kernel page table after running userspace on this CPU.
        pub fn activate_kernel_paging() {}

        /// Flush TLB by the associated `vaddr`, or flush the entire TLB. (`vaddr` is `None`).
        pub(crate) fn flush_tlb(vaddr: Option<VirtAddr>);

        /// Clone kernel space entries (top level only) from `src` page table to `dst` page table.
        pub(crate) fn pt_clone_kernel_space(dst_pt_root: PhysAddr, src_pt_root: PhysAddr);
    }

    /// Interrupts management.
    pub mod interrupt {
        /// Suspend the CPU (also enable interrupts) and wait for an interrupt
        /// to occurs, then disable interrupts.
        pub fn wait_for_interrupt() {
            core::hint::spin_loop();
        }

        /// Is a valid IRQ number.
        pub fn is_valid_irq(vector: usize) -> bool;

        /// Enable the interrupts
        pub fn intr_on();

        /// Disable the interrupts
        pub fn intr_off();

        /// Test weather interrupt is enabled
        pub fn intr_get() -> bool;

        /// Disable IRQ.
        pub fn mask_irq(vector: usize) -> HalResult;

        /// Enable IRQ.
        pub fn unmask_irq(vector: usize) -> HalResult;

        /// Configure the specified interrupt vector. If it is invoked, it must be
        /// invoked prior to interrupt registration.
        pub fn configure_irq(vector: usize, tm: IrqTriggerMode, pol: IrqPolarity) -> HalResult;

        /// Add an interrupt handler to an IRQ.
        pub fn register_irq_handler(vector: usize, handler: IrqHandler) -> HalResult;

        /// Remove the interrupt handler to an IRQ.
        pub fn unregister_irq_handler(vector: usize) -> HalResult;

        /// Handle IRQ.
        pub fn handle_irq(vector: usize);

        /// Method used for platform allocation of blocks of MSI and MSI-X compatible
        /// IRQ targets.
        pub fn msi_alloc_block(requested_irqs: usize) -> HalResult<Range<usize>>;

        /// Method used to free a block of MSI IRQs previously allocated by msi_alloc_block().
        /// This does not unregister IRQ handlers.
        pub fn msi_free_block(block: Range<usize>) -> HalResult;

        /// Register a handler function for a given msi_id within an msi_block_t. Passing a
        /// NULL handler will effectively unregister a handler for a given msi_id within the
        /// block.
        pub fn msi_register_handler(block: Range<usize>, msi_id: usize, handler: IrqHandler) -> HalResult;

        pub fn send_ipi(cpuid: usize, reason: usize) -> HalResult;

        /// Send a pure wake IPI to a halted CPU: no queue entry, no TLB work —
        /// the target's IPI handler sees an empty queue and simply returns,
        /// having broken the `hlt`. Used by the scheduler's reschedule kick.
        /// Default: no-op (arches without an implementation keep the old
        /// behaviour of waking on the next periodic tick).
        pub fn send_wake_ipi(_cpuid: usize) {}

        pub fn ipi_reason() -> Vec<usize>;
    }

    pub mod console {
        pub fn console_write_early(_s: &str) {}
        pub fn console_progress_early(_progress: u32) {}
        /// Last-resort panic banner drawn raw to the framebuffer (no locks, no
        /// alloc). No-op where there is no direct framebuffer access.
        pub fn console_panic_banner(_s: &str) {}
    }

    /// Thread spawning.
    pub mod thread: common::thread {
        /// Spawn a new thread.
        pub fn spawn(future: impl Future<Output = ()> + Send + 'static);

        /// Spawn a new thread bound to a CPU affinity mask.
        ///
        /// Bit `i` of `affinity` set means the thread may run on logical CPU `i`.
        /// The mask is shared so it can be updated at runtime (e.g. via
        /// `sched_setaffinity`); the scheduler observes changes on its next
        /// placement or work-stealing decision. On hosted (libos) builds the
        /// mask is ignored.
        pub fn spawn_with_affinity(future: impl Future<Output = ()> + Send + 'static, affinity: Arc<core::sync::atomic::AtomicU64>);

        /// Set tid and pid of current task.
        pub fn set_current_thread(thread: Option<Arc<dyn Any + Send + Sync>>) {}

        /// Get tid and pid of current task.
        pub fn get_current_thread() -> Option<Arc<dyn Any + Send + Sync>> { None }

        /// Consume this CPU's pending *wake-up preemption* request, if any.
        ///
        /// A task that became runnable on this CPU while a different thread was
        /// occupying it sets the request; the trap path calls this and yields so
        /// the woken task does not have to wait out the running thread's whole
        /// timeslice. Returns `true` exactly once per request. Hosted (libos)
        /// builds run on the host scheduler and always return `false`.
        pub fn take_need_resched() -> bool { false }

        /// Instantaneous run-queue length across every CPU: tasks queued ready
        /// to run plus the task each CPU is polling right now (Linux's
        /// `nr_running`). The caller, if it is itself an executor task, is
        /// included. Hosted (libos) builds have no visibility into the host
        /// scheduler and return 0.
        pub fn runnable_task_count() -> usize { 0 }
    }

    /// Time and clock functions.
    pub mod timer {
        /// Set the first time interrupt
        pub fn timer_enable();

        /// Get current time.
        /// TODO: use `Instant` as return type.
        pub fn timer_now() -> Duration;

        /// Converting from now-relative durations to absolute deadlines.
        pub fn deadline_after(dur: Duration) -> Duration {
            timer_now() + dur
        }

        /// Set a new timer. After `deadline`, the `callback` will be called.
        /// TODO: use `Instant` as the type of `deadline`.
        pub fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>);

        /// Check timers, call when timer interrupt happened.
        pub(crate) fn timer_tick();

        /// Tickless-idle hook: called on a CPU that is about to halt with no
        /// runnable work. Implementations may stretch the periodic timer to the
        /// next pending deadline (capped) so a fully idle CPU stops taking the
        /// full-rate scheduler tick. Default: no-op (keep the periodic tick).
        pub fn timer_idle_enter() {}

        /// Tickless-idle hook: called when a CPU resumes real work, to restore
        /// the full-rate scheduler tick stretched by [`timer_idle_enter`].
        /// Default: no-op.
        pub fn timer_idle_exit() {}
    }

    /// Random number generator.
    pub mod rand {
        /// Fill random bytes to the buffer
        #[allow(unused_variables)]
        pub fn fill_random(buf: &mut [u8]) {
            cfg_if! {
                if #[cfg(target_arch = "x86_64")] {
                    // One RDRAND yields 8 bytes; issuing it per *byte* made
                    // /dev/urandom ~1000 cycles/byte, which stalls library
                    // startups that read entropy in bulk.
                    for chunk in buf.chunks_mut(8) {
                        let mut r: u64 = 0;
                        unsafe { core::arch::x86_64::_rdrand64_step(&mut r) };
                        chunk.copy_from_slice(&r.to_ne_bytes()[..chunk.len()]);
                    }
                } else {
                    static mut SEED: u64 = 0xdead_beef_cafe_babe;
                    for x in buf.iter_mut() {
                        unsafe {
                            // from musl
                            SEED = SEED.wrapping_mul(0x5851_f42d_4c95_7f2d);
                            *x = (SEED >> 33) as u8;
                        }
                    }
                }
            }
        }
    }

    /// VDSO constants.
    pub mod vdso: common::vdso {
        /// Get platform specific information.
        pub fn vdso_constants() -> VdsoConstants {
            vdso_constants_template()
        }
    }
}
