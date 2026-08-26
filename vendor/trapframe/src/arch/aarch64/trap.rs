use super::*;
use core::arch::{asm, global_asm};

global_asm!(include_str!("trap.S"));

/// Initialize interrupt handling for the current HART.
///
/// # Safety
///
/// This function will:
/// - Set `vbar_el1` to internal exception vector.
///
/// You **MUST NOT** modify these registers later.
pub unsafe fn init() {
    // Set the exception vector address
    asm!("msr VBAR_EL1, {}", in(reg) __vectors as *const () as usize);
}

/// Trap frame of kernel interrupt
///
/// # Trap handler
///
/// You need to define a handler function like this:
///
/// ```no_run
/// use trapframe::TrapFrame;
///
/// #[no_mangle]
/// pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
///     println!("TRAP! tf: {:#x?}", tf);
/// }
/// ```
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    /// Trap num: Source and Kind
    pub trap_num: usize,
    /// Reserved for internal use
    pub __reserved: usize,
    /// Exception Link Register, elr_el1
    pub elr: usize,
    /// Saved Process Status Register, spsr_el1
    pub spsr: usize,
    /// Stack Pointer, sp_el1
    pub sp: usize,
    /// Software Thread ID Register, tpidr_el1
    pub tpidr: usize,
    /// General registers
    /// Must be last in this struct
    pub general: GeneralRegs,
}

impl UserContext {
    /// Go to user space with the context, and come back when a trap occurs.
    ///
    /// On return, the context will be reset to the status before the trap.
    /// Trap reason and error code will be returned.
    ///
    /// # Example
    /// ```no_run
    /// use trapframe::{UserContext, GeneralRegs};
    ///
    /// // init user space context
    /// let mut context = UserContext {
    ///     general: GeneralRegs {
    ///         ..Default::default()
    ///     },
    ///     sp: 0x10000,
    ///     elr: 0x1000,
    ///     ..Default::default()
    /// };
    /// // go to user
    /// context.run();
    /// // back from user
    /// println!("back from user: {:#x?}", context);
    /// ```
    pub fn run(&mut self) {
        unsafe { run_user(self) }
    }
}

#[allow(improper_ctypes)]
extern "C" {
    fn __vectors();
    fn run_user(regs: &mut UserContext);
}
