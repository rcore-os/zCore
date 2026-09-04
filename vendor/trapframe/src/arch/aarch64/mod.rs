//! AArch64 register layouts and context-switch entry points.

#[cfg(target_os = "linux")]
mod fncall;
#[cfg(any(target_os = "none", target_os = "uefi"))]
mod trap;

#[cfg(target_os = "linux")]
pub use fncall::*;
#[cfg(any(target_os = "none", target_os = "uefi"))]
pub use trap::*;

/// Saved AArch64 user context used to enter and resume user code.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct UserContext {
    /// Encoded exception source and kind.
    pub trap_num: usize,
    /// Reserved for the assembly frame layout.
    pub __reserved: usize,
    /// Exception Link Register (`ELR_EL1`).
    pub elr: usize,
    /// Saved Process Status Register (`SPSR_EL1`).
    pub spsr: usize,
    /// User stack pointer (`SP_EL0`).
    pub sp: usize,
    /// User thread pointer (`TPIDR_EL0`).
    pub tpidr: usize,
    /// General-purpose registers; kept last for the assembly layout.
    pub general: GeneralRegs,
}

/// An AArch64 user context that also preserves floating-point and SIMD state.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C, align(16))]
pub struct UserContextWithExtensions {
    /// Encoded exception source and kind.
    pub trap_num: usize,
    /// Reserved for the assembly frame layout.
    pub __reserved: usize,
    /// Exception Link Register (`ELR_EL1`).
    pub elr: usize,
    /// Saved Process Status Register (`SPSR_EL1`).
    pub spsr: usize,
    /// User stack pointer (`SP_EL0`).
    pub sp: usize,
    /// User thread pointer (`TPIDR_EL0`).
    pub tpidr: usize,
    /// General-purpose registers.
    pub general: GeneralRegs,
    /// Floating-point and Advanced SIMD state.
    pub fp_simd: FpSimdState,
}

impl core::ops::Deref for UserContextWithExtensions {
    type Target = UserContext;

    fn deref(&self) -> &Self::Target {
        // The base fields are an identical `repr(C)` prefix.
        unsafe { &*(self as *const Self).cast() }
    }
}

impl core::ops::DerefMut for UserContextWithExtensions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // The base fields are an identical `repr(C)` prefix.
        unsafe { &mut *(self as *mut Self).cast() }
    }
}

/// Saved AArch64 floating-point and Advanced SIMD state.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C, align(16))]
pub struct FpSimdState {
    /// SIMD and floating-point registers Q0 through Q31.
    pub registers: [u128; 32],
    /// Floating-point control register.
    pub fpcr: u32,
    /// Floating-point status register.
    pub fpsr: u32,
}

/// AArch64 general-purpose registers.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct GeneralRegs {
    /// General-purpose register X1.
    pub x1: usize,
    /// General-purpose register X2.
    pub x2: usize,
    /// General-purpose register X3.
    pub x3: usize,
    /// General-purpose register X4.
    pub x4: usize,
    /// General-purpose register X5.
    pub x5: usize,
    /// General-purpose register X6.
    pub x6: usize,
    /// General-purpose register X7.
    pub x7: usize,
    /// Indirect result location register X8; also the Linux syscall number.
    pub x8: usize,
    /// General-purpose register X9.
    pub x9: usize,
    /// General-purpose register X10.
    pub x10: usize,
    /// General-purpose register X11.
    pub x11: usize,
    /// General-purpose register X12.
    pub x12: usize,
    /// General-purpose register X13.
    pub x13: usize,
    /// General-purpose register X14.
    pub x14: usize,
    /// General-purpose register X15.
    pub x15: usize,
    /// Intra-procedure-call scratch register X16.
    pub x16: usize,
    /// Intra-procedure-call scratch register X17.
    pub x17: usize,
    /// Platform register X18.
    pub x18: usize,
    /// Callee-saved register X19.
    pub x19: usize,
    /// Callee-saved register X20.
    pub x20: usize,
    /// Callee-saved register X21.
    pub x21: usize,
    /// Callee-saved register X22.
    pub x22: usize,
    /// Callee-saved register X23.
    pub x23: usize,
    /// Callee-saved register X24.
    pub x24: usize,
    /// Callee-saved register X25.
    pub x25: usize,
    /// Callee-saved register X26.
    pub x26: usize,
    /// Callee-saved register X27.
    pub x27: usize,
    /// Callee-saved register X28.
    pub x28: usize,
    /// Frame pointer register X29.
    pub x29: usize,
    /// Reserved alignment slot used by the assembly layout.
    pub __reserved: usize,
    /// Link register X30.
    pub x30: usize,
    /// Argument and return-value register X0.
    pub x0: usize,
}

impl UserContext {
    /// Returns the Linux system call number from `x8`.
    pub fn get_syscall_num(&self) -> usize {
        self.general.x8
    }

    /// Returns the system call result from `x0`.
    pub fn get_syscall_ret(&self) -> usize {
        self.general.x0
    }

    /// Sets the system call result in `x0`.
    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.general.x0 = ret;
    }

    /// Returns the six system call arguments in ABI order.
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

    /// Sets the exception return address.
    pub fn set_ip(&mut self, ip: usize) {
        self.elr = ip;
    }

    /// Sets the user stack pointer.
    pub fn set_sp(&mut self, sp: usize) {
        self.sp = sp;
    }

    /// Returns the user stack pointer.
    pub fn get_sp(&self) -> usize {
        self.sp
    }

    /// Sets the user TLS pointer.
    pub fn set_tls(&mut self, tls: usize) {
        self.tpidr = tls;
    }
}
