use alloc::boxed::Box;
use core::{fmt::Arguments, future::Future, ops::Range, pin::Pin, time::Duration};

use crate::{common, HalResult, PhysAddr, VirtAddr};

hal_fn_def! {
    pub mod cpu {
        /// Current CPU ID.
        pub fn cpu_id() -> u8 { 0 }

        /// Current CPU frequency.
        pub fn cpu_frequency() -> u16 { 3000 }
    }

    pub mod mem: common::mem {
        /// Convert physical address to virtual address.
        pub(crate) fn phys_to_virt(paddr: PhysAddr) -> VirtAddr;

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

        /// Allocate one physical frame.
        pub(crate) fn frame_alloc() -> Option<PhysAddr>;

        /// Allocate contiguous `frame_count` physical frames.
        pub(crate) fn frame_alloc_contiguous(frame_count: usize, align_log2: usize) -> Option<PhysAddr>;

        /// Deallocate a physical frame.
        pub(crate) fn frame_dealloc(paddr: PhysAddr);
    }

    pub mod vm: common::vm {
        /// Activate this page table by given `vmtoken`.
        pub(crate) fn activate_paging(vmtoken: PhysAddr);

        /// Read current VM token. (e.g. CR3, SATP, ...)
        pub(crate) fn current_vmtoken() -> PhysAddr;

        /// Flush TLB by the associated `vaddr`, or flush the entire TLB. (`vaddr` is `None`).
        pub(crate) fn flush_tlb(vaddr: Option<VirtAddr>);

        /// Clone kernel space entries (top level only) from `src` page table to `dst` page table.
        pub(crate) fn pt_clone_kernel_space(dst_pt_root: PhysAddr, src_pt_root: PhysAddr);
    }

    pub mod interrupt {
        /// Enable IRQ.
        pub fn enable_irq(vector: u32);

        /// Disable IRQ.
        pub fn disable_irq(vector: u32);

        /// Is a valid IRQ number.
        pub fn is_valid_irq(vector: u32) -> bool;

        /// Configure the specified interrupt vector.  If it is invoked, it muust be
        /// invoked prior to interrupt registration.
        pub fn configure_irq(vector: u32, trig_mode: bool, polarity: bool) -> HalResult;

        /// Add an interrupt handle to an IRQ
        pub fn register_irq_handler(vector: u32, handler: Box<dyn Fn() + Send + Sync>) -> HalResult<u32>;

        /// Remove the interrupt handle to an IRQ
        pub fn unregister_irq_handler(vector: u32) -> HalResult;

        /// Handle IRQ.
        pub fn handle_irq(vector: u32);

        /// Method used for platform allocation of blocks of MSI and MSI-X compatible
        /// IRQ targets.
        pub fn msi_allocate_block(requested_irqs: u32) -> HalResult<Range<u32>>;

        /// Method used to free a block of MSI IRQs previously allocated by msi_alloc_block().
        /// This does not unregister IRQ handlers.
        pub fn msi_free_block(block: Range<u32>) -> HalResult;

        /// Register a handler function for a given msi_id within an msi_block_t. Passing a
        /// NULL handler will effectively unregister a handler for a given msi_id within the
        /// block.
        pub fn msi_register_handler(block: Range<u32>, msi_id: u32, handler: Box<dyn Fn() + Send + Sync>) -> HalResult;
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

        /// Get fault address of the last page fault.
        pub fn fetch_fault_vaddr() -> VirtAddr;

        /// Get the trap number when trap.
        pub fn fetch_trap_num(context: &UserContext) -> usize;
    }

    pub mod thread {
        /// Spawn a new thread.
        pub fn spawn(future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>, vmtoken: usize);

        /// Set tid and pid of current task.
        pub fn set_tid(tid: u64, pid: u64);

        /// Get tid and pid of current task.]
        pub fn get_tid() -> (u64, u64);
    }

    pub mod timer {
        /// Get current time.
        pub fn timer_now() -> Duration;

        /// Set a new timer. After `deadline`, the `callback` will be called.
        pub fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>);

        /// Check timers, call when timer interrupt happened.
        pub fn timer_tick();
    }

    pub mod serial {
        /// Register a callback of serial readable event.
        pub fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>);

        /// Put a char to serial buffer.
        pub fn serial_put(x: u8);

        /// Read a string from serial buffer.
        pub fn serial_read(buf: &mut [u8]) -> usize;

        /// Print format string and its arguments to serial.
        pub fn serial_write_fmt(fmt: Arguments);

        /// Print a string to serial.
        pub fn serial_write(s: &str) {
            serial_write_fmt(format_args!("{}", s));
        }
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
                        unsafe {
                            core::arch::x86_64::_rdrand16_step(&mut r);
                        }
                        *x = r as _;
                    }
                } else {
                    unimplemented!()
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

pub mod dev {
    use super::*;

    hal_fn_def! {
        pub mod fb: common::fb {
            /// Initialize framebuffer.
            pub fn init();
        }

        pub mod input {
            /// Initialize input devices.
            pub fn init();

            /// Setup the callback when a keyboard event occurs.
            pub fn kbd_set_callback(callback: Box<dyn Fn(u16, i32) + Send + Sync>);

            /// Setup the callback when a mouse event occurs.
            pub fn mouse_set_callback(callback: Box<dyn Fn([u8; 3]) + Send + Sync>);
        }
    }
}
