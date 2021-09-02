use alloc::boxed::Box;
use core::{fmt::Arguments, future::Future, pin::Pin, time::Duration};

use crate::{common, PhysAddr, VirtAddr};

hal_fn_def! {
    pub mod cpu {
        /// Current CPU ID.
        fn cpu_id() -> u8 { 0 }

        /// Current CPU frequency.
        fn cpu_frequency() -> u16 { 3000 }
    }

    pub mod mem: common::mem {
        /// Read physical memory from `paddr` to `buf`.
        fn pmem_read(paddr: PhysAddr, buf: &mut [u8]);

        /// Write physical memory to `paddr` from `buf`.
        fn pmem_write(paddr: PhysAddr, buf: &[u8]);

        /// Zero physical memory at `[paddr, paddr + len)`.
        fn pmem_zero(paddr: PhysAddr, len: usize);

        /// Copy content of `src` frame to `target` frame.
        fn frame_copy(src: PhysAddr, target: PhysAddr);

        /// Flush the physical frame.
        fn frame_flush(target: PhysAddr);

        /// Allocate one physical frame.
        fn frame_alloc() -> Option<PhysAddr>;

        /// Allocate contiguous `frame_count` physical frames.
        fn frame_alloc_contiguous(frame_count: usize, align_log2: usize) -> Option<PhysAddr>;

        /// Deallocate a physical frame.
        fn frame_dealloc(paddr: PhysAddr);

        /// Get the physical frame contains all zeros.
        fn zero_frame_addr() -> PhysAddr;
    }

    pub mod interrupt {
        /// Enable IRQ.
        fn enable_irq(vector: u32);

        /// Disable IRQ.
        fn disable_irq(vector: u32);

        /// Is a valid IRQ number.
        fn is_valid_irq(vector: u32) -> bool;

        /// Configure the specified interrupt vector.  If it is invoked, it muust be
        /// invoked prior to interrupt registration.
        fn configure_irq(vector: u32, trig_mode: bool, polarity: bool) -> bool;

        /// Add an interrupt handle to an IRQ
        fn register_irq_handler(vector: u32, handler: Box<dyn Fn() + Send + Sync>) -> Option<u32>;

        /// Remove the interrupt handle to an IRQ
        fn unregister_irq_handler(vector: u32) -> bool;

        /// Handle IRQ.
        fn handle_irq(vector: u32);

        /// Method used for platform allocation of blocks of MSI and MSI-X compatible
        /// IRQ targets.
        fn msi_allocate_block(irq_num: u32) -> Option<(usize, usize)>;

        /// Method used to free a block of MSI IRQs previously allocated by msi_alloc_block().
        /// This does not unregister IRQ handlers.
        fn msi_free_block(irq_start: u32, irq_num: u32);

        /// Register a handler function for a given msi_id within an msi_block_t. Passing a
        /// NULL handler will effectively unregister a handler for a given msi_id within the
        /// block.
        fn msi_register_handler(irq_start: u32, irq_num: u32, msi_id: u32, handler: Box<dyn Fn() + Send + Sync>);
    }

    pub mod context: common::context {
        /// Enter user mode.
        fn context_run(context: &mut UserContext);

        /// Get fault address of the last page fault.
        fn fetch_fault_vaddr() -> VirtAddr;

        /// Get the trap number when trap.
        fn fetch_trap_num(context: &UserContext) -> usize;
    }

    pub mod thread {
        /// Spawn a new thread.
        fn spawn(future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>, vmtoken: usize);

        /// Set tid and pid of current task.
        fn set_tid(tid: u64, pid: u64);

        /// Get tid and pid of current task.]
        fn get_tid() -> (u64, u64);
    }

    pub mod timer {
        /// Get current time.
        fn timer_now() -> Duration;

        /// Set a new timer. After `deadline`, the `callback` will be called.
        fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>);

        /// Check timers, call when timer interrupt happened.
        fn timer_tick();
    }

    pub mod serial {
        /// Register a callback of serial readable event.
        fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>);

        /// Put a char to serial buffer.
        fn serial_put(x: u8);

        /// Read a string from serial buffer.
        fn serial_read(buf: &mut [u8]) -> usize;

        /// Print format string and its arguments to console.
        fn print_fmt(fmt: Arguments);

        /// Print a string to console.
        fn print_str(s: &str) {
            print_fmt(format_args!("{}", s));
        }
    }

    pub mod rand {
        /// Fill random bytes to the buffer
        #[allow(unused_variables)]
        fn fill_random(buf: &mut [u8]) {
            cfg_if::cfg_if! {
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
        fn vdso_constants() -> VdsoConstants {
            vdso_constants_template()
        }
    }
}

pub mod dev {
    use super::*;

    hal_fn_def! {
        pub mod fb: common::fb {
            /// Initialize framebuffer.
            fn init();
        }

        pub mod input {
            /// Initialize input devices.
            fn init();

            /// Setup the callback when a keyboard event occurs.
            fn kbd_set_callback(callback: Box<dyn Fn(u16, i32) + Send + Sync>);

            /// Setup the callback when a mouse event occurs.
            fn mouse_set_callback(callback: Box<dyn Fn([u8; 3]) + Send + Sync>);
        }
    }
}
