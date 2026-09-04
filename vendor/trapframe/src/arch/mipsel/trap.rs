use core::arch::{asm, global_asm};

global_asm!(include_str!("trap.S"));

/// Initializes exception handling on the current processor.
///
/// # Safety
///
/// This function will:
/// - Set CP0 `EBase` to the internal exception vector.
/// - Clear CP0 Status `BEV` to enable that vector.
///
/// You **MUST NOT** modify these registers later.
pub unsafe fn init() {
    let status: usize;
    unsafe {
        // Set CP0 EBase (15, 1) to the trap entry and select that vector by
        // clearing the bootstrap exception vector bit in CP0 Status.
        asm!(
            "mtc0 $2, $15, 1",
            "mfc0 $3, $12",
            in("$2") trap_entry,
            out("$3") status,
        );
        let status = status & !(1 << 22);
        asm!("mtc0 $2, $12", "ehb", in("$2") status);
    }
}

/// Register frame saved when an exception enters the kernel.
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
    /// User thread-local storage pointer.
    pub tls: usize,
    /// Reserved for the assembly frame layout.
    pub __reserved: usize,
    /// CP0 Status register.
    pub status: usize,
    /// CP0 Cause register.
    pub cause: usize,
    /// CP0 exception program counter.
    pub epc: usize,
    /// CP0 bad virtual address.
    pub vaddr: usize,
    /// General-purpose registers.
    pub general: GeneralRegs,
}

/// Saved MIPS user context used to enter and resume user code.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct UserContext {
    /// User thread-local storage pointer.
    pub tls: usize,
    /// Reserved for the assembly frame layout.
    pub __reserved: usize,
    /// CP0 Status register.
    pub status: usize,
    /// CP0 Cause register.
    pub cause: usize,
    /// CP0 exception program counter.
    pub epc: usize,
    /// CP0 bad virtual address.
    pub vaddr: usize,
    /// General-purpose registers.
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
    ///         sp: 0x10000,
    ///         ..Default::default()
    ///     },
    ///     epc: 0x1000,
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

/// MIPS integer registers and multiply/divide result registers.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct GeneralRegs {
    /// High multiply/divide result register.
    pub hi: usize,
    /// Low multiply/divide result register.
    pub lo: usize,
    /// Assembler temporary register `$1`.
    pub at: usize,
    /// Return value and system call number register `$2`.
    pub v0: usize,
    /// Return value register `$3`.
    pub v1: usize,
    /// Argument register `$4`.
    pub a0: usize,
    /// Argument register `$5`.
    pub a1: usize,
    /// Argument register `$6`.
    pub a2: usize,
    /// Argument or error-indicator register `$7`.
    pub a3: usize,
    /// Temporary register `$8`.
    pub t0: usize,
    /// Temporary register `$9`.
    pub t1: usize,
    /// Temporary register `$10`.
    pub t2: usize,
    /// Temporary register `$11`.
    pub t3: usize,
    /// Temporary register `$12`.
    pub t4: usize,
    /// Temporary register `$13`.
    pub t5: usize,
    /// Temporary register `$14`.
    pub t6: usize,
    /// Temporary register `$15`.
    pub t7: usize,
    /// Saved register `$16`.
    pub s0: usize,
    /// Saved register `$17`.
    pub s1: usize,
    /// Saved register `$18`.
    pub s2: usize,
    /// Saved register `$19`.
    pub s3: usize,
    /// Saved register `$20`.
    pub s4: usize,
    /// Saved register `$21`.
    pub s5: usize,
    /// Saved register `$22`.
    pub s6: usize,
    /// Saved register `$23`.
    pub s7: usize,
    /// Temporary register `$24`.
    pub t8: usize,
    /// Temporary register `$25`.
    pub t9: usize,
    /// Kernel-reserved register `$26`.
    pub k0: usize,
    /// Kernel-reserved register `$27`.
    pub k1: usize,
    /// Global pointer register `$28`.
    pub gp: usize,
    /// Stack pointer register `$29`.
    pub sp: usize,
    /// Frame pointer register `$30`.
    pub fp: usize,
    /// Return address register `$31`.
    pub ra: usize,
}

impl UserContext {
    /// Returns the system call number from `v0`.
    pub fn get_syscall_num(&self) -> usize {
        self.general.v0
    }

    /// Returns the n32 ABI system call result.
    pub fn get_syscall_ret(&self) -> usize {
        // MIPS n32 abi
        if self.general.a3 == 0 {
            self.general.v0
        } else {
            (-(self.general.v0 as isize)) as usize
        }
    }

    /// Sets the n32 ABI system call result and error indicator.
    pub fn set_syscall_ret(&mut self, ret: usize) {
        // MIPS n32 abi
        if (ret as isize) < 0 {
            self.general.v0 = (-(ret as isize)) as usize;
            self.general.a3 = 1;
        } else {
            self.general.v0 = ret as usize;
            self.general.a3 = 0;
        }
    }

    /// Returns the six system call arguments in ABI order.
    pub fn get_syscall_args(&self) -> [usize; 6] {
        [
            self.general.a0,
            self.general.a1,
            self.general.a2,
            self.general.a3,
            self.general.t0,
            self.general.t1,
        ]
    }

    /// Sets the exception return address.
    pub fn set_ip(&mut self, ip: usize) {
        self.epc = ip;
    }

    /// Sets the user stack pointer.
    pub fn set_sp(&mut self, sp: usize) {
        self.general.sp = sp;
    }

    /// Returns the user stack pointer.
    pub fn get_sp(&self) -> usize {
        self.general.sp
    }

    /// Sets the user thread-local storage pointer.
    pub fn set_tls(&mut self, tls: usize) {
        self.tls = tls;
    }
}

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn trap_entry();
    fn run_user(regs: &mut UserContext);
}
