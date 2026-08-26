use core::arch::{asm, global_asm};

global_asm!(include_str!("trap.S"));

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
    // Set cp0 ebase(15, 1) register to trap entry
    asm!(
        "mtc0 {trap_entry}, $15, 1",
        trap_entry = in(reg) trap_entry,
    );
}

#[no_mangle]
#[linkage = "weak"]
extern "C" fn trap_handler(tf: &mut TrapFrame) {
    unimplemented!("TRAP: tf={:#x?}", tf);
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
    /// TLS
    pub tls: usize,
    /// Reserved for internal use
    pub __reserved: usize,
    /// CP0 Status
    pub status: usize,
    /// CP0 cause
    pub cause: usize,
    /// CP0 epc
    pub epc: usize,
    /// CP0 vaddr
    pub vaddr: usize,
    /// General registers
    pub general: GeneralRegs,
}

/// Saved registers on a trap.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct UserContext {
    /// TLS
    pub tls: usize,
    /// Reserved for internal use
    pub __reserved: usize,
    /// CP0 Status
    pub status: usize,
    /// CP0 cause
    pub cause: usize,
    /// CP0 epc
    pub epc: usize,
    /// CP0 vaddr
    pub vaddr: usize,
    /// General registers
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

/// General registers
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct GeneralRegs {
    pub hi: usize,
    pub lo: usize,
    pub at: usize,
    pub v0: usize,
    pub v1: usize,
    pub a0: usize,
    pub a1: usize,
    pub a2: usize,
    pub a3: usize,
    pub t0: usize,
    pub t1: usize,
    pub t2: usize,
    pub t3: usize,
    pub t4: usize,
    pub t5: usize,
    pub t6: usize,
    pub t7: usize,
    pub s0: usize,
    pub s1: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub t8: usize,
    pub t9: usize,
    pub k0: usize,
    pub k1: usize,
    pub gp: usize,
    pub sp: usize,
    pub fp: usize,
    pub ra: usize,
}

impl UserContext {
    /// Get number of syscall
    pub fn get_syscall_num(&self) -> usize {
        self.general.v0
    }

    /// Get return value of syscall
    pub fn get_syscall_ret(&self) -> usize {
        // MIPS n32 abi
        if self.general.a3 == 0 {
            self.general.v0
        } else {
            (-(self.general.v0 as isize)) as usize
        }
    }

    /// Set return value of syscall
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

    /// Get syscall args
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

    /// Set instruction pointer
    pub fn set_ip(&mut self, ip: usize) {
        self.epc = ip;
    }

    /// Set stack pointer
    pub fn set_sp(&mut self, sp: usize) {
        self.general.sp = sp;
    }

    /// Get stack pointer
    pub fn get_sp(&self) -> usize {
        self.general.sp
    }

    /// Set tls pointer
    pub fn set_tls(&mut self, tls: usize) {
        self.tls = tls;
    }
}

#[allow(improper_ctypes)]
extern "C" {
    fn trap_entry();
    fn run_user(regs: &mut UserContext);
}
