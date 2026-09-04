use core::arch::{asm, global_asm};

#[cfg(target_arch = "riscv32")]
global_asm!(
    r"
    .equ XLENB, 4
    .macro LOAD_SP a1, a2
        lw \a1, \a2*XLENB(sp)
    .endm
    .macro STORE_SP a1, a2
        sw \a1, \a2*XLENB(sp)
    .endm
",
    include_str!("trap.S")
);
#[cfg(target_arch = "riscv64")]
global_asm!(
    r"
    .equ XLENB, 8
    .equ RISCV_EXTENDED_CONTEXT, 1
    .macro LOAD_SP a1, a2
        ld \a1, \a2*XLENB(sp)
    .endm
    .macro STORE_SP a1, a2
        sd \a1, \a2*XLENB(sp)
    .endm
",
    include_str!("trap.S")
);

/// Initialize interrupt handling for the current HART.
///
/// # Safety
///
/// This function will:
/// - Set `sscratch` to 0.
/// - Set `stvec` to internal exception vector.
///
/// You **MUST NOT** modify these registers later.
pub unsafe fn init() {
    unsafe {
        // Set sscratch register to 0, indicating to exception vector that we are
        // presently executing in the kernel
        asm!("csrw sscratch, zero");
        // Set the exception vector address
        asm!("csrw stvec, {}", in(reg) trap_entry as *const () as usize);
    }
}

/// Register frame saved when a trap enters the kernel.
///
/// # Trap handler
///
/// You need to define a handler function like this:
///
/// ```no_run
/// use trapframe::TrapFrame;
///
/// #[unsafe(no_mangle)]
/// pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
///     println!("TRAP! tf: {:#x?}", tf);
/// }
/// ```
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    /// General-purpose registers.
    pub general: GeneralRegs,
    /// Supervisor status register (`sstatus`).
    pub sstatus: usize,
    /// Supervisor exception program counter (`sepc`).
    pub sepc: usize,
}

/// Saved RISC-V user context used to enter and resume user code.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct UserContext {
    /// General-purpose registers.
    pub general: GeneralRegs,
    /// Supervisor status register (`sstatus`).
    pub sstatus: usize,
    /// Supervisor exception program counter (`sepc`).
    pub sepc: usize,
}

/// A RISC-V 64 user context that also preserves floating-point and vector state.
#[cfg(target_arch = "riscv64")]
#[derive(Debug, Default, Clone, Copy)]
#[repr(C, align(16))]
pub struct UserContextWithExtensions {
    /// Integer registers.
    pub general: GeneralRegs,
    /// Supervisor status register (`sstatus`).
    pub sstatus: usize,
    /// Supervisor exception program counter (`sepc`).
    pub sepc: usize,
    /// RISC-V vector registers and control state.
    pub vector: VectorRegs,
    /// RISC-V floating-point registers and control state.
    pub float: FloatRegs,
}

#[cfg(target_arch = "riscv64")]
impl core::ops::Deref for UserContextWithExtensions {
    type Target = UserContext;

    fn deref(&self) -> &Self::Target {
        // The base fields are an identical `repr(C)` prefix.
        unsafe { &*(self as *const Self).cast() }
    }
}

#[cfg(target_arch = "riscv64")]
impl core::ops::DerefMut for UserContextWithExtensions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // The base fields are an identical `repr(C)` prefix.
        unsafe { &mut *(self as *mut Self).cast() }
    }
}

/// Saved state for a RISC-V V implementation with VLEN=128.
#[cfg(target_arch = "riscv64")]
#[derive(Debug, Default, Clone, Copy)]
#[repr(C, align(16))]
pub struct VectorRegs {
    /// The 32 vector registers, each 128 bits wide.
    pub registers: [u128; 32],
    /// Vector start index.
    pub vstart: usize,
    /// Vector length.
    pub vl: usize,
    /// Vector type.
    pub vtype: usize,
    /// Fixed-point rounding mode and saturation flag.
    pub vcsr: usize,
}

/// Saved double-precision RISC-V floating-point state.
#[cfg(target_arch = "riscv64")]
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct FloatRegs {
    /// Floating-point registers F0 through F31.
    pub registers: [u64; 32],
    /// Floating-point control and status register.
    pub fcsr: usize,
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
    ///         sp: 0x10000,
    ///         ..Default::default()
    ///     },
    ///     sepc: 0x1000,
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

#[cfg(target_arch = "riscv64")]
impl UserContextWithExtensions {
    /// Runs user code while preserving floating-point and vector state.
    pub fn run(&mut self) {
        unsafe { run_user_extended(self) }
    }
}

/// RISC-V integer registers in architectural order.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct GeneralRegs {
    /// Hard-wired zero register X0.
    pub zero: usize,
    /// Return address register X1.
    pub ra: usize,
    /// Stack pointer register X2.
    pub sp: usize,
    /// Global pointer register X3.
    pub gp: usize,
    /// Thread pointer register X4.
    pub tp: usize,
    /// Temporary register X5.
    pub t0: usize,
    /// Temporary register X6.
    pub t1: usize,
    /// Temporary register X7.
    pub t2: usize,
    /// Saved/frame pointer register X8.
    pub s0: usize,
    /// Saved register X9.
    pub s1: usize,
    /// Argument and return-value register X10.
    pub a0: usize,
    /// Argument and return-value register X11.
    pub a1: usize,
    /// Argument register X12.
    pub a2: usize,
    /// Argument register X13.
    pub a3: usize,
    /// Argument register X14.
    pub a4: usize,
    /// Argument register X15.
    pub a5: usize,
    /// Argument register X16.
    pub a6: usize,
    /// Argument and system call number register X17.
    pub a7: usize,
    /// Saved register X18.
    pub s2: usize,
    /// Saved register X19.
    pub s3: usize,
    /// Saved register X20.
    pub s4: usize,
    /// Saved register X21.
    pub s5: usize,
    /// Saved register X22.
    pub s6: usize,
    /// Saved register X23.
    pub s7: usize,
    /// Saved register X24.
    pub s8: usize,
    /// Saved register X25.
    pub s9: usize,
    /// Saved register X26.
    pub s10: usize,
    /// Saved register X27.
    pub s11: usize,
    /// Temporary register X28.
    pub t3: usize,
    /// Temporary register X29.
    pub t4: usize,
    /// Temporary register X30.
    pub t5: usize,
    /// Temporary register X31.
    pub t6: usize,
}

impl UserContext {
    /// Returns the system call number from `a7`.
    pub fn get_syscall_num(&self) -> usize {
        self.general.a7
    }

    /// Returns the system call result from `a0`.
    pub fn get_syscall_ret(&self) -> usize {
        self.general.a0
    }

    /// Sets the system call result in `a0`.
    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.general.a0 = ret;
    }

    /// Returns the six system call arguments in ABI order.
    pub fn get_syscall_args(&self) -> [usize; 6] {
        [
            self.general.a0,
            self.general.a1,
            self.general.a2,
            self.general.a3,
            self.general.a4,
            self.general.a5,
        ]
    }

    /// Sets the exception return address.
    pub fn set_ip(&mut self, ip: usize) {
        self.sepc = ip;
    }

    /// Sets the user stack pointer.
    pub fn set_sp(&mut self, sp: usize) {
        self.general.sp = sp;
    }

    /// Returns the user stack pointer.
    pub fn get_sp(&self) -> usize {
        self.general.sp
    }

    /// Sets the user thread pointer.
    pub fn set_tls(&mut self, tls: usize) {
        self.general.tp = tls;
    }
}

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn trap_entry();
    fn run_user(regs: &mut UserContext);
    #[cfg(target_arch = "riscv64")]
    fn run_user_extended(regs: &mut UserContextWithExtensions);
}
