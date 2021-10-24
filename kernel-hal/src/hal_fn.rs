use alloc::{boxed::Box, string::String, vec::Vec};
use core::{future::Future, ops::Range, pin::Pin, time::Duration};

use crate::drivers::prelude::{IrqHandler, IrqPolarity, IrqTriggerMode};
use crate::{common, HalResult, KernelConfig, KernelHandler, MMUFlags, PhysAddr, VirtAddr};

hal_fn_def! {
    pub mod boot {
        /// The kernel command line.
        pub fn cmdline() -> String { "".into() }

        /// Returns the slice of the initial RAM disk, or `None` if not exist.
        pub fn init_ram_disk() -> Option<&'static mut [u8]> {
            None
        }

        /// Initialize the primary CPU at an early stage (before the physical frame allocator).
        #[doc(cfg(feature = "smp"))]
        pub fn primary_init_early(cfg: KernelConfig, handler: &'static impl KernelHandler) {}

        /// The main part of the primary CPU initialization.
        #[doc(cfg(feature = "smp"))]
        pub fn primary_init();

        /// Initialize the secondary CPUs.
        #[doc(cfg(feature = "smp"))]
        pub fn secondary_init() {}
    }

    pub mod cpu {
        /// Current CPU ID.
        pub fn cpu_id() -> u8 { 0 }

        /// Current CPU frequency.
        pub fn cpu_frequency() -> u16 { 3000 }
    }

    pub mod mem: common::mem {
        /// Convert physical address to virtual address.
        pub(crate) fn phys_to_virt(paddr: PhysAddr) -> VirtAddr;

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
    }

    pub mod vm: common::vm {
        /// Read current VM token. (e.g. CR3, SATP, ...)
        pub fn current_vmtoken() -> PhysAddr;

        /// Activate this page table by given `vmtoken`.
        pub(crate) fn activate_paging(vmtoken: PhysAddr);

        /// Flush TLB by the associated `vaddr`, or flush the entire TLB. (`vaddr` is `None`).
        pub(crate) fn flush_tlb(vaddr: Option<VirtAddr>);

        /// Clone kernel space entries (top level only) from `src` page table to `dst` page table.
        pub(crate) fn pt_clone_kernel_space(dst_pt_root: PhysAddr, src_pt_root: PhysAddr);
    }

    pub mod interrupt {
        /// Suspend the CPU (also enable interrupts) and wait for an interrupt
        /// to occurs, then disable interrupts.
        pub fn wait_for_interrupt() {
            core::hint::spin_loop();
        }

        /// Is a valid IRQ number.
        pub fn is_valid_irq(vector: usize) -> bool;

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
    }

    pub(crate) mod console {
        pub(crate) fn console_write_early(_s: &str) {}
    }

    pub mod context: common::context {
        /// Enter user mode.
        pub fn context_run(context: &mut UserContext) {
            cfg_if! {
                if #[cfg(feature = "libos")] {
                    context.run_fncall()
                } else {
                    context.run()
                }
            }
        }

        /// Get the trap number when trap.
        pub fn fetch_trap_num(context: &UserContext) -> usize;

        /// Get the fault virtual address and access type of the last page fault by `info_reg`
        /// (`error_code` for x86, `scause` for riscv).
        pub fn fetch_page_fault_info(info_reg: usize) -> (VirtAddr, MMUFlags);
    }

    pub mod thread: common::thread {
        /// Spawn a new thread.
        pub fn spawn(future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>, vmtoken: usize);

        /// Set tid and pid of current task.
        pub fn set_tid(tid: u64, pid: u64);

        /// Get tid and pid of current task.]
        pub fn get_tid() -> (u64, u64);
    }

    pub mod timer {
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
    }

    pub mod rand {
        /// Fill random bytes to the buffer
        #[allow(unused_variables)]
        pub fn fill_random(buf: &mut [u8]) {
            cfg_if! {
                if #[cfg(target_arch = "x86_64")] {
                    // TODO: optimize
                    for x in buf.iter_mut() {
                        let mut r = 0;
                        unsafe { core::arch::x86_64::_rdrand16_step(&mut r) };
                        *x = r as _;
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

    pub mod vdso: common::vdso {
        /// Get platform specific information.
        pub fn vdso_constants() -> VdsoConstants {
            vdso_constants_template()
        }
    }
}
