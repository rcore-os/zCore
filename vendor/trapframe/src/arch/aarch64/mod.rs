#[cfg(target_os = "linux")]
mod fncall;
#[cfg(any(target_os = "none", target_os = "uefi"))]
mod trap;

#[cfg(target_os = "linux")]
pub use fncall::*;
#[cfg(any(target_os = "none", target_os = "uefi"))]
pub use trap::*;

/// Saved registers on a trap.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct UserContext {
    /// Trap num: Source and Kind
    pub trap_num: usize,
    /// Reserved for internal use
    pub __reserved: usize,
    /// Exception Link Register, elr_el1
    pub elr: usize,
    /// Saved Process Status Register, spsr_el1
    pub spsr: usize,
    /// Stack Pointer, sp_el0
    pub sp: usize,
    /// Software Thread ID Register, tpidr_el0
    pub tpidr: usize,
    /// General registers
    /// Must be last in this struct
    pub general: GeneralRegs,
}

/// General registers
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct GeneralRegs {
    pub x1: usize,
    pub x2: usize,
    pub x3: usize,
    pub x4: usize,
    pub x5: usize,
    pub x6: usize,
    pub x7: usize,
    pub x8: usize,
    pub x9: usize,
    pub x10: usize,
    pub x11: usize,
    pub x12: usize,
    pub x13: usize,
    pub x14: usize,
    pub x15: usize,
    pub x16: usize,
    pub x17: usize,
    pub x18: usize,
    pub x19: usize,
    pub x20: usize,
    pub x21: usize,
    pub x22: usize,
    pub x23: usize,
    pub x24: usize,
    pub x25: usize,
    pub x26: usize,
    pub x27: usize,
    pub x28: usize,
    pub x29: usize,
    pub __reserved: usize, // for alignment
    pub x30: usize,
    // put here deliberately for ease of asm
    pub x0: usize,
    // x31 means special
}

impl UserContext {
    /// Get number of syscall
    pub fn get_syscall_num(&self) -> usize {
        self.general.x8
    }

    /// Get return value of syscall
    pub fn get_syscall_ret(&self) -> usize {
        self.general.x0
    }

    /// Set return value of syscall
    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.general.x0 = ret;
    }

    /// Get syscall args
    pub fn get_syscall_args(&self) -> [usize; 6] {
        [
            self.general.x0,
            self.general.x1,
            self.general.x2,
            self.general.x3,
            self.general.x4,
            self.general.x5,
        ]
    }

    /// Set instruction pointer
    pub fn set_ip(&mut self, ip: usize) {
        self.elr = ip;
    }

    /// Set stack pointer
    pub fn set_sp(&mut self, sp: usize) {
        self.sp = sp;
    }

    /// Get stack pointer
    pub fn get_sp(&self) -> usize {
        self.sp
    }

    /// Set tls pointer
    pub fn set_tls(&mut self, tls: usize) {
        self.tpidr = tls;
    }
}
