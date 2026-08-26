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
#[derive(Debug, PartialEq, Eq)]
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

#[cfg(not(feature = "libos"))]
pub const TIMER_INTERRUPT_VEC: usize = crate::timer_interrupt_vector();

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
                } else if !code.contains(PageFaultErrorCode::INST) {
                    // Instruction-fetch faults (INST=1) require EXECUTE
                    // permission, not READ. Only set READ for genuine data
                    // reads (WRITE=0 and INST=0).  Mixing in READ for
                    // INST faults overly restricts the vmar permission check
                    // (which tests `mapping_flags.contains(access_flags)`)
                    // and produces a misleading "flags=READ | EXECUTE" in
                    // the null-range #PF diagnostic.
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

    #[cfg(target_arch = "aarch64")]
    pub fn from(esr: usize) -> Self {
        // TODO: check if is right
        use crate::{Fault, Info, Kind, Source, Syndrome};
        use cortex_a::registers::{ESR_EL1, FAR_EL1};
        use tock_registers::interfaces::Readable;

        let info = Info {
            source: Source::from(esr & 0xffff),
            kind: Kind::from((esr >> 16) & 0xffff),
        };
        let esr = ESR_EL1.get() as u32;
        match info.kind {
            Kind::Synchronous => match Syndrome::from(esr) {
                Syndrome::Breakpoint => Self::SoftwareBreakpoint,
                Syndrome::Svc(_) => Self::Syscall,
                Syndrome::DataAbort { kind: _, level: _ } => Self::PageFault(
                    FAR_EL1.get() as _,
                    MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER,
                ),
                Syndrome::InstructionAbort {
                    kind: Fault::Permission,
                    level: _,
                } => Self::PageFault(FAR_EL1.get() as _, MMUFlags::EXECUTE | MMUFlags::USER),
                Syndrome::PCAlignmentFault | Syndrome::SpAlignmentFault => Self::UnalignedAccess,
                _ => Self::GernelFault(esr as usize),
            },
            Kind::Irq => Self::Interrupt(
                #[cfg(not(feature = "libos"))]
                {
                    use crate::hal_fn::mem::phys_to_virt;
                    use crate::KCONFIG;
                    zcore_drivers::irq::gic_400::get_irq_num(
                        phys_to_virt(KCONFIG.gic_base + 0x1_0000),
                        phys_to_virt(KCONFIG.gic_base),
                    )
                },
                #[cfg(feature = "libos")]
                {
                    // TODO: interrupt in libOS
                    usize::MAX
                },
            ),
            _ => Self::GernelFault(esr as usize),
        }
    }
}

/// User context saved on trap.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct UserContext(UserContextInner);

/// DEBUG: dirección donde el asm de trap guardó el último `GeneralRegs` (x86_64
/// bare). Se compara con [`UserContext::dbg_ctx_addr`].
#[cfg(all(target_arch = "x86_64", not(feature = "libos")))]
pub fn dbg_asm_save_addr() -> usize {
    trapframe::dbg_save_addr()
}
#[cfg(not(all(target_arch = "x86_64", not(feature = "libos"))))]
pub fn dbg_asm_save_addr() -> usize {
    0
}

impl UserContext {
    /// Create an empty user context.
    pub fn new() -> Self {
        let context = UserContextInner::default();
        Self(context)
    }

    /// Initialize the context for entry into userspace.
    /// Note: if the number of args < 3, please fill with zeros
    /// Eg: ctx.setup_uspace(pc_, sp_, &[arg1, arg2, 0])
    pub fn setup_uspace(&mut self, pc: usize, sp: usize, args: &[usize; 3]) {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                self.0.general.rip = pc;
                self.0.general.rsp = sp;
                self.0.general.rdi = args[0];
                self.0.general.rsi = args[1];
                self.0.general.rdx = args[2];
                // IOPL = 3, IF = 1
                // FIXME: set IOPL = 0 when IO port bitmap is supporte
                self.0.general.rflags = 0x3000 | 0x200 | 0x2;
            } else if #[cfg(target_arch = "aarch64")] {
                self.0.elr = pc;
                self.0.sp = sp;
                self.0.general.x0 = args[0];
                self.0.general.x1 = args[1];
                self.0.general.x2 = args[2];
                // Mask SError exceptions (currently unhandled).
                // TODO
                self.0.spsr = 1 << 8;
            } else if #[cfg(target_arch = "riscv64")] {
                self.0.sepc = pc;
                self.0.general.sp = sp;
                self.0.general.a0 = args[0];
                self.0.general.a1 = args[1];
                self.0.general.a2 = args[2];
                // SUM = 1, FS = 0b11, SPIE = 1
                self.0.sstatus = 1 << 18 | 0b11 << 13 | 1 << 5;
            }
        }
    }

    /// Setup return addr
    pub fn set_ra(&mut self, _ra: usize) {
        cfg_if! {
            if #[cfg(target_arch = "riscv64")] {
                self.0.general.ra = _ra;
            } else if #[cfg(target_arch = "x86_64")] {
                error!("Please set return addr via stack!");
            } else if #[cfg(target_arch = "aarch64")] {
                self.0.general.x30 = _ra;
            } else {
                unimplemented!("Unsupported arch!");
            }
        }
    }

    /// Switch to user mode.
    pub fn enter_uspace(&mut self) {
        cfg_if! {
            if #[cfg(feature = "libos")] {
                self.0.run_fncall()
            } else {
                self.dbg_validate_user_ctx("before enter_uspace");
                // [rbpfix] Break the physmap-rbp propagation loop. A user thread
                // intermittently ends up with `rbp` = physmap base (bit 47 set,
                // sign-extended) OR'd over its real, small frame pointer — i.e.
                // `phys_to_virt(rbp)`. A hardware data-write watchpoint pinned the
                // propagation to `trap_syscall_entry`'s `push %rbp`: the corrupt
                // value is saved into the context on every syscall/interrupt and
                // restored into the register on every return (`pop rbp`), so once
                // a thread's rbp is physmap it stays physmap — and the thread
                // livelocks (re-enters, faults on the bogus frame pointer,
                // re-enters…), killing multithreaded processes like Firefox.
                //
                // A user rbp in the kernel physmap window is always wrong (ring 3
                // can never legitimately hold it), so mask bits 47-63 off before
                // entering user mode. That restores the real low frame pointer and
                // breaks the loop at the one point every return funnels through.
                // Exactly targeted: legitimate low frame pointers and the System V
                // outermost-frame `rbp = -1` sentinel are outside the window and
                // untouched. (The one-time seed that first sets the bit is a rare
                // register-level event, not a memory write; this makes it benign.)
                #[cfg(target_arch = "x86_64")]
                {
                    const PHYSMAP_BASE: usize = 0xffff_8000_0000_0000;
                    const PHYSMAP_END: usize = 0xffff_c000_0000_0000;
                    let rbp = self.0.general.rbp;
                    if (PHYSMAP_BASE..PHYSMAP_END).contains(&rbp) {
                        let fixed = rbp & 0x0000_7fff_ffff_ffff;
                        error!(
                            "[rbpfix] sanitizing corrupt user rbp {:#x} -> {:#x}",
                            rbp, fixed
                        );
                        self.0.general.rbp = fixed;
                    }
                }
                self.0.run();
                self.dbg_validate_user_ctx("after trap from user");
            }
        }
    }

    /// DEBUG instrumentation: catch a corrupted saved user context (rip/rsp/rbp
    /// pointing into the kernel half) right before we (re)enter user mode and
    /// right after a trap returns. A "before" hit means the saved `GeneralRegs`
    /// were corrupted while sitting in kernel memory (or at save time); pairing
    /// it with the "after" hit localizes the intermittent register-state
    /// corruption behind the `apk` SIGSEGV / "BAD signature".
    #[inline]
    fn dbg_validate_user_ctx(&self, when: &str) {
        #[cfg(all(target_arch = "x86_64", not(feature = "libos")))]
        {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static HB: AtomicUsize = AtomicUsize::new(0);
            let n = HB.fetch_add(1, Ordering::Relaxed);
            if n % 200_000 == 0 {
                warn!("[ctxcheck] heartbeat n={} ({})", n, when);
            }
            const USER_MAX: usize = 0x0000_8000_0000_0000;
            // Kernel direct-map ("physmap") window. A *leaked* kernel pointer in
            // a user general register lands here; that is the corruption we hunt
            // (see the `[uleak]` scanner in `user.rs`).
            const PHYSMAP_BASE: usize = 0xffff_8000_0000_0000;
            const PHYSMAP_END: usize = 0xffff_c000_0000_0000;
            // Do NOT treat a low rip/rsp/rbp as corruption: this loader maps PIE
            // executables (apk and the musl dynamic linker `ld-musl`) at base 0
            // (see `Loader::load_impl`: `app_base` is the start of an empty
            // address space, i.e. 0), so genuinely-valid user code, stack and
            // frame pointers legitimately live at very low virtual addresses —
            // a running PIE has rip well below the old 0x1_0000 floor. The
            // earlier `rip < USER_PC_MIN` heuristic therefore mis-fired on every
            // asynchronous trap taken while such a process ran low. The dump that
            // exposed this had trap_num=0xf3 (the IPI vector — an *interrupt*,
            // not a page fault), which proves the CPU was executing normally at
            // that low rip when the IPI arrived; a wild jump to an unmapped low
            // address would have raised #PF (trap_num=0xe) instead.
            //
            // The only reliable corruption signal is a user register pointing
            // into the *kernel* half (>= USER_MAX) — e.g. a user rbp that ended
            // up inside the physmap window (0xffff_8000_…) — so keep just that.
            let g = &self.0.general;
            // `rip` and `rsp` are consumed by the CPU on `sysret`/`iret`, so a
            // kernel-half value there is genuinely fatal. `rbp` (and every other
            // GPR) is never dereferenced by the kernel and may legitimately hold
            // a non-canonical sentinel — e.g. the System V outermost-frame
            // marker `rbp = -1` (0xffff_ffff_ffff_ffff), which a process exiting
            // via `exit_group` routinely carries. Flagging `rbp >= USER_MAX`
            // therefore mis-fired on every such exit. Only treat `rbp` as
            // corrupt when it is an actual leaked physmap pointer.
            let rbp_leak = (PHYSMAP_BASE..PHYSMAP_END).contains(&g.rbp);
            if g.rip >= USER_MAX || g.rsp >= USER_MAX || rbp_leak {
                error!(
                    "[ctxcheck] {} CORRUPT user ctx cpu={} ctx_addr={:#x}: rip={:#x} rsp={:#x} rbp={:#x} fsbase={:#x} \
                     rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} \
                     r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x} \
                     rflags={:#x} trap_num={:#x} err={:#x}",
                    when,
                    crate::cpu::cpu_id(),
                    core::ptr::addr_of!(self.0.general) as usize,
                    g.rip, g.rsp, g.rbp, g.fsbase,
                    g.rax, g.rbx, g.rcx, g.rdx, g.rsi, g.rdi,
                    g.r8, g.r9, g.r10, g.r11, g.r12, g.r13, g.r14, g.r15,
                    g.rflags, self.0.trap_num, self.0.error_code,
                );
            }
        }
        let _ = when;
    }

    /// DEBUG: leer el `rbp` del contexto de usuario guardado (frame pointer en
    /// x86_64). Usado por el handler de page-fault para comparar el `rbp` guardado
    /// con la dirección que falló.
    pub fn dbg_general_rbp(&self) -> usize {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                self.0.general.rbp
            } else {
                0
            }
        }
    }

    /// DEBUG: dirección en memoria del `GeneralRegs` de este `UserContext`. Se
    /// compara con [`dbg_asm_save_addr`] (donde el asm guardó de verdad) para
    /// detectar si el save/restore usan memorias distintas.
    pub fn dbg_ctx_addr(&self) -> usize {
        core::ptr::addr_of!(self.0.general) as usize
    }

    /// DEBUG: registros del bucle de histograma de apk para ver cuál sostiene el
    /// puntero corrupto `physmap`. Devuelve (rsi, rdi, r8, r9, r10, r11).
    pub fn dbg_loop_regs(&self) -> (usize, usize, usize, usize, usize, usize) {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                let g = &self.0.general;
                (g.rsi, g.rdi, g.r8, g.r9, g.r10, g.r11)
            } else {
                (0, 0, 0, 0, 0, 0)
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
                TrapReason::from(self.0.trap_num)
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
