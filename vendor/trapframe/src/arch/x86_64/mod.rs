#[cfg(any(target_os = "linux", target_os = "macos"))]
mod fncall;
#[cfg(any(target_os = "none", target_os = "uefi"))]
mod gdt;
#[cfg(any(target_os = "none", target_os = "uefi"))]
mod idt;
#[cfg(feature = "ioport_bitmap")]
#[cfg(any(target_os = "none", target_os = "uefi"))]
pub mod ioport;
#[cfg(any(target_os = "none", target_os = "uefi"))]
mod syscall;
#[cfg(any(target_os = "none", target_os = "uefi"))]
mod trap;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use fncall::syscall_fn_entry;
#[cfg(any(target_os = "none", target_os = "uefi"))]
pub use trap::TrapFrame;

/// Initialize interrupt handling on x86_64.
///
/// # Safety
///
/// This function will:
///
/// - Disable interrupt.
/// - Switch to a new [GDT], extend 7 more entries from the current one.
/// - Switch to a new [TSS], set `GSBASE` to its base address.
/// - Switch to a new [IDT], override the current one.
/// - Enable [`syscall`] instruction.
///     - set `EFER::SYSTEM_CALL_EXTENSIONS`
///
/// [GDT]: https://wiki.osdev.org/GDT
/// [IDT]: https://wiki.osdev.org/IDT
/// [TSS]: https://wiki.osdev.org/Task_State_Segment
/// [`syscall`]: https://www.felixcloutier.com/x86/syscall
///
#[cfg(any(target_os = "none", target_os = "uefi"))]
pub unsafe fn init() {
    x86_64::instructions::interrupts::disable();
    gdt::init();
    idt::init();
    syscall::init();
}

/// Saved x86-64 user context used to enter and resume user code.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct UserContext {
    /// General-purpose registers and execution state.
    pub general: GeneralRegs,
    /// Interrupt vector or synthetic trap number that returned control.
    pub trap_num: usize,
    /// Error code associated with the trap.
    pub error_code: usize,
}

/// An x86-64 user context that also preserves x87 and SSE state.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C, align(16))]
pub struct UserContextWithExtensions {
    /// General-purpose registers and execution state.
    pub general: GeneralRegs,
    /// Interrupt vector or synthetic trap number that returned control.
    pub trap_num: usize,
    /// Error code associated with the trap.
    pub error_code: usize,
    /// x87 and SSE state.
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

/// The 512-byte, 16-byte-aligned x87/SSE save area used by FXSAVE.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[repr(C, align(16))]
pub struct FpSimdState {
    /// Raw architectural FXSAVE image.
    pub bytes: [u8; 512],
}

impl Default for FpSimdState {
    fn default() -> Self {
        let mut bytes = [0; 512];
        // Architectural reset values for the x87 control word and MXCSR.
        bytes[0] = 0x7f;
        bytes[1] = 0x03;
        bytes[24] = 0x80;
        bytes[25] = 0x1f;
        Self { bytes }
    }
}

/// x86-64 general-purpose registers and execution state.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct GeneralRegs {
    /// Accumulator register.
    pub rax: usize,
    /// Base register.
    pub rbx: usize,
    /// Counter register.
    pub rcx: usize,
    /// Data register.
    pub rdx: usize,
    /// Source index register.
    pub rsi: usize,
    /// Destination index register.
    pub rdi: usize,
    /// Frame pointer register.
    pub rbp: usize,
    /// Stack pointer register.
    pub rsp: usize,
    /// General-purpose register R8.
    pub r8: usize,
    /// General-purpose register R9.
    pub r9: usize,
    /// General-purpose register R10.
    pub r10: usize,
    /// General-purpose register R11.
    pub r11: usize,
    /// General-purpose register R12.
    pub r12: usize,
    /// General-purpose register R13.
    pub r13: usize,
    /// General-purpose register R14.
    pub r14: usize,
    /// General-purpose register R15.
    pub r15: usize,
    /// Instruction pointer.
    pub rip: usize,
    /// Saved processor flags.
    pub rflags: usize,
    /// FS segment base, conventionally used as the user TLS pointer.
    pub fsbase: usize,
    /// GS segment base.
    pub gsbase: usize,
}

impl UserContext {
    /// Returns the system call number from `rax`.
    pub fn get_syscall_num(&self) -> usize {
        self.general.rax
    }

    /// Returns the system call result from `rax`.
    pub fn get_syscall_ret(&self) -> usize {
        self.general.rax
    }

    /// Sets the system call result in `rax`.
    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.general.rax = ret;
    }

    /// Returns the six system call arguments in ABI order.
    pub fn get_syscall_args(&self) -> [usize; 6] {
        [
            self.general.rdi,
            self.general.rsi,
            self.general.rdx,
            self.general.r10,
            self.general.r8,
            self.general.r9,
        ]
    }

    /// Sets the instruction pointer.
    pub fn set_ip(&mut self, ip: usize) {
        self.general.rip = ip;
    }

    /// Sets the stack pointer.
    pub fn set_sp(&mut self, sp: usize) {
        self.general.rsp = sp;
    }

    /// Returns the stack pointer.
    pub fn get_sp(&self) -> usize {
        self.general.rsp
    }

    /// Sets the user TLS pointer in `fsbase`.
    pub fn set_tls(&mut self, tls: usize) {
        self.general.fsbase = tls;
    }
}
