//! User context.

use crate::{MMUFlags, VirtAddr};
use core::fmt;
use trapframe::UserContext as UserContextInner;

pub use trapframe::GeneralRegs;

cfg_if! {
    if #[cfg(feature = "libos")] {
        pub use trapframe::syscall_fn_entry as syscall_entry;
    } else {
        pub use dummpy_syscall_entry as syscall_entry;
        pub fn dummpy_syscall_entry() {
            unreachable!("dummpy_syscall_entry")
        }
    }
}

/// For reading and writing fields in [`UserContext`].
#[derive(Debug)]
pub enum UserContextField {
    InstrPointer,
    StackPointer,
    ThreadPointer,
    ReturnValue,
}

/// Reason of the trap.
#[derive(Debug)]
pub enum TrapReason {
    Syscall,
    Interrupt(usize),
    PageFault(VirtAddr, MMUFlags),
    UndefinedInstruction,
    SoftwareBreakpoint,
    HardwareBreakpoint,
    UnalignedAccess,
    GernelFault(usize),
}

impl TrapReason {
    /// Get [`TrapReason`] from `trap_num` and `error_code` in trap frame for x86.
    #[cfg(target_arch = "x86_64")]
    pub fn from(trap_num: usize, error_code: usize) -> Self {
        use x86::irq::*;
        const X86_INT_BASE: u8 = 0x20;
        const X86_INT_MAX: u8 = 0xff;

        // See https://github.com/rcore-os/trapframe-rs/blob/25cb5282aca8ceb4f7fc4dcd61e7e73b67d9ae00/src/arch/x86_64/syscall.S#L117
        if trap_num == 0x100 {
            return Self::Syscall;
        }
        match trap_num as u8 {
            DEBUG_VECTOR => Self::HardwareBreakpoint,
            BREAKPOINT_VECTOR => Self::SoftwareBreakpoint,
            INVALID_OPCODE_VECTOR => Self::UndefinedInstruction,
            ALIGNMENT_CHECK_VECTOR => Self::UnalignedAccess,
            PAGE_FAULT_VECTOR => {
                bitflags::bitflags! {
                    struct PageFaultErrorCode: u32 {
                        const PRESENT =     1 << 0;
                        const WRITE =       1 << 1;
                        const USER =        1 << 2;
                        const RESERVED =    1 << 3;
                        const INST =        1 << 4;
                    }
                }
                let fault_vaddr = x86_64::registers::control::Cr2::read().as_u64() as _;
                let code = PageFaultErrorCode::from_bits_truncate(error_code as u32);
                let mut flags = MMUFlags::empty();
                if code.contains(PageFaultErrorCode::WRITE) {
                    flags |= MMUFlags::WRITE
                } else {
                    flags |= MMUFlags::READ
                }
                if code.contains(PageFaultErrorCode::USER) {
                    flags |= MMUFlags::USER
                }
                if code.contains(PageFaultErrorCode::INST) {
                    flags |= MMUFlags::EXECUTE
                }
                if code.contains(PageFaultErrorCode::RESERVED) {
                    error!("page table entry has reserved bits set!");
                }
                Self::PageFault(fault_vaddr, flags)
            }
            vec @ X86_INT_BASE..=X86_INT_MAX => Self::Interrupt(vec as usize),
            _ => Self::GernelFault(trap_num),
        }
    }

    #[cfg(target_arch = "riscv64")]
    pub fn from(scause: riscv::register::scause::Scause) -> Self {
        use riscv::register::scause::{Exception, Trap};
        let stval = riscv::register::stval::read();
        match scause.cause() {
            Trap::Exception(Exception::UserEnvCall) => Self::Syscall,
            Trap::Exception(Exception::Breakpoint) => Self::SoftwareBreakpoint,
            Trap::Exception(Exception::IllegalInstruction) => Self::UndefinedInstruction,
            Trap::Exception(Exception::InstructionMisaligned)
            | Trap::Exception(Exception::StoreMisaligned) => Self::UnalignedAccess,
            Trap::Exception(Exception::LoadPageFault) => Self::PageFault(stval, MMUFlags::READ),
            Trap::Exception(Exception::StorePageFault) => Self::PageFault(stval, MMUFlags::WRITE),
            Trap::Exception(Exception::InstructionPageFault) => {
                Self::PageFault(stval, MMUFlags::EXECUTE)
            }
            Trap::Interrupt(_) => Self::Interrupt(scause.code()),
            _ => Self::GernelFault(scause.code()),
        }
    }
}

/// User context saved on trap.
#[repr(transparent)]
#[derive(Clone)]
pub struct UserContext(UserContextInner);

impl UserContext {
    /// Create an empty user context.
    pub fn new() -> Self {
        let context = UserContextInner::default();
        Self(context)
    }

    /// Initialize the context for entry into userspace.
    pub fn setup_uspace(&mut self, pc: usize, sp: usize, arg1: usize, arg2: usize) {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                self.0.general.rip = pc;
                self.0.general.rsp = sp;
                self.0.general.rdi = arg1;
                self.0.general.rsi = arg2;
                // IOPL = 3, IF = 1
                // FIXME: set IOPL = 0 when IO port bitmap is supporte
                self.0.general.rflags = 0x3000 | 0x200 | 0x2;
            } else if #[cfg(target_arch = "aarch64")] {
                self.0.elr = pc;
                self.0.sp = sp;
                self.0.general.x0 = arg1;
                self.0.general.x1 = arg2;
                // Mask SError exceptions (currently unhandled).
                // TODO
                self.0.spsr = 1 << 8;
            } else if #[cfg(target_arch = "riscv64")] {
                self.0.sepc = pc;
                self.0.general.sp = sp;
                self.0.general.a0 = arg1;
                self.0.general.a1 = arg2;
                // SUM = 1, FS = 0b11, SPIE = 1
                self.0.sstatus = 1 << 18 | 0b11 << 13 | 1 << 5;
            }
        }
    }

    /// Switch to user mode.
    pub fn enter_uspace(&mut self) {
        cfg_if! {
            if #[cfg(feature = "libos")] {
                self.0.run_fncall()
            } else {
                self.0.run()
            }
        }
    }

    /// Returns the `error_code` field of the context.
    #[cfg(any(target_arch = "x86_64", doc))]
    #[doc(cfg(target_arch = "x86_64"))]
    pub fn error_code(&self) -> usize {
        self.0.error_code
    }

    /// Returns [`TrapReason`] according to the context.
    pub fn trap_reason(&self) -> TrapReason {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                TrapReason::from(self.0.trap_num, self.0.error_code)
            } else if #[cfg(target_arch = "aarch64")] {
                unimplemented!() // ESR_EL1
            } else if #[cfg(target_arch = "riscv64")] {
                TrapReason::from(riscv::register::scause::read())
            } else {
                unimplemented!()
            }
        }
    }
    /// Returns a `usize` representing the trap reason. (i.e., IDT vector for x86, `scause` for RISC-V)
    pub fn raw_trap_reason(&self) -> usize {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                self.0.trap_num
            } else if #[cfg(target_arch = "aarch64")] {
                unimplemented!() // ESR_EL1
            } else if #[cfg(target_arch = "riscv64")] {
                riscv::register::scause::read().bits()
            } else {
                unimplemented!()
            }
        }
    }

    /// Returns the reference of general registers.
    pub fn general(&self) -> &GeneralRegs {
        &self.0.general
    }

    /// Returns the mutable reference of general registers.
    pub fn general_mut(&mut self) -> &mut GeneralRegs {
        &mut self.0.general
    }

    fn field_ref(&mut self, which: UserContextField) -> &mut usize {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                match which {
                    UserContextField::InstrPointer => &mut self.0.general.rip,
                    UserContextField::StackPointer => &mut self.0.general.rsp,
                    UserContextField::ThreadPointer => &mut self.0.general.fsbase,
                    UserContextField::ReturnValue => &mut self.0.general.rax,
                }
            } else if #[cfg(target_arch = "aarch64")] {
                match which {
                    UserContextField::InstrPointer => &mut self.0.elr,
                    UserContextField::StackPointer => &mut self.0.sp,
                    UserContextField::ThreadPointer => &mut self.0.tpidr,
                    UserContextField::ReturnValue => &mut self.0.general.x0,
                }
            } else if #[cfg(target_arch = "riscv64")] {
                match which {
                    UserContextField::InstrPointer => &mut self.0.sepc,
                    UserContextField::StackPointer => &mut self.0.general.sp,
                    UserContextField::ThreadPointer => &mut self.0.general.tp,
                    UserContextField::ReturnValue => &mut self.0.general.a0,
                }
            } else {
                unimplemented!()
            }
        }
    }

    /// Read a field of the context.
    pub fn get_field(&mut self, which: UserContextField) -> usize {
        *self.field_ref(which)
    }

    /// Write a field of the context.
    pub fn set_field(&mut self, which: UserContextField, value: usize) {
        *self.field_ref(which) = value;
    }

    /// Advance the instruction pointer in trap handler on some architecture.
    pub fn advance_pc(&mut self, reason: TrapReason) {
        cfg_if! {
            if #[cfg(target_arch = "riscv64")] {
                if let TrapReason::Syscall = reason { self.0.sepc += 4 }
            } else {
                let _ = reason;
            }
        }
    }
}

impl Default for UserContext {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for UserContext {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        /// X86 vector registers.
        #[repr(C, align(16))]
        #[derive(Debug, Copy, Clone)]
        pub struct VectorRegs {
            pub fcw: u16,
            pub fsw: u16,
            pub ftw: u8,
            pub _pad0: u8,
            pub fop: u16,
            pub fip: u32,
            pub fcs: u16,
            pub _pad1: u16,

            pub fdp: u32,
            pub fds: u16,
            pub _pad2: u16,
            pub mxcsr: u32,
            pub mxcsr_mask: u32,

            pub mm: [U128; 8],
            pub xmm: [U128; 16],
            pub reserved: [U128; 3],
            pub available: [U128; 3],
        }

        // https://xem.github.io/minix86/manual/intel-x86-and-64-manual-vol1/o_7281d5ea06a5b67a-274.html
        impl Default for VectorRegs {
            fn default() -> Self {
                VectorRegs {
                    fcw: 0x37f,
                    mxcsr: 0x1f80,
                    ..unsafe { core::mem::zeroed() }
                }
            }
        }

        // workaround: libcore has bug on Debug print u128 ??
        /// Unsigned 128-bit
        #[derive(Default, Clone, Copy)]
        #[repr(C, align(16))]
        pub struct U128(pub [u64; 2]);

        impl fmt::Debug for U128 {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{:#016x}_{:016x}", self.0[1], self.0[0])
            }
        }
    }
}
