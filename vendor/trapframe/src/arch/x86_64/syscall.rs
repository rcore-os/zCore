use super::UserContext;
use core::arch::global_asm;
use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

/// DEBUG: dirección base (per-CPU) donde `trap_syscall_entry` guardó el último
/// `GeneralRegs` en esta CPU. El asm la escribe en la región GS; aquí se lee.
pub fn dbg_save_addr() -> usize {
    super::gdt::read_dbg_save()
}

global_asm!(
    include_str!("syscall.S"),
    DBG_OFF = const super::gdt::DBG_SAVE_GS_OFFSET,
);

pub fn init() {
    let cpuid = raw_cpuid::CpuId::new();
    unsafe {
        // enable `syscall` instruction
        assert!(cpuid
            .get_extended_processor_and_feature_identifiers()
            .unwrap()
            .has_syscall_sysret());
        Efer::update(|efer| {
            efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        });

        // flags to clear on syscall
        // copy from Linux 5.0
        // TF|DF|IF|IOPL|AC|NT
        const RFLAGS_MASK: u64 = 0x47700;

        LStar::write(VirtAddr::new(syscall_entry as *const () as usize as u64));
        SFMask::write(RFlags::from_bits(RFLAGS_MASK).unwrap());
    }
}

extern "sysv64" {
    fn syscall_entry();
    fn syscall_return(regs: &mut UserContext);
}

impl UserContext {
    /// Go to user space with the context, and come back when a trap occurs.
    ///
    /// On return, the context will be reset to the status before the trap.
    /// Trap reason and error code will be placed at `trap_num` and `error_code`.
    ///
    /// If the trap was triggered by `syscall` instruction, the `trap_num` will be set to `0x100`.
    ///
    /// If `trap_num` is `0x100`, it will go user by `sysret` (`rcx` and `r11` are dropped),
    /// otherwise it will use `iret`.
    ///
    /// # Example
    /// ```no_run
    /// use trapframe::{UserContext, GeneralRegs};
    ///
    /// // init user space context
    /// let mut context = UserContext {
    ///     general: GeneralRegs {
    ///         rip: 0x1000,
    ///         rsp: 0x10000,
    ///         ..Default::default()
    ///     },
    ///     ..Default::default()
    /// };
    /// // go to user
    /// context.run();
    /// // back from user
    /// println!("back from user: {:#x?}", context);
    /// ```
    pub fn run(&mut self) {
        unsafe {
            // Restore this thread's user FPU/SSE state immediately before entering
            // user mode, and save it immediately after the trap returns. The
            // syscall_return / syscall_entry asm paths use no SSE, and this Rust
            // wrapper has no float work, so XMM cannot be clobbered in between.
            // `fpstate` is 16-aligned (FXSAVE requirement).
            let fp = core::ptr::addr_of_mut!(self.fpstate) as *mut u8;
            core::arch::asm!("fxrstor [{}]", in(reg) fp, options(readonly, nostack, preserves_flags));
            syscall_return(self);
            let fp = core::ptr::addr_of_mut!(self.fpstate) as *mut u8;
            core::arch::asm!("fxsave [{}]", in(reg) fp, options(nostack, preserves_flags));
        }
    }
}
