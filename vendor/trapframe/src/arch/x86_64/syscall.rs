use super::{UserContext, UserContextWithExtensions};
use core::arch::global_asm;
use x86_64::VirtAddr;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask};
use x86_64::registers::rflags::RFlags;

global_asm!(include_str!("syscall.S"));

pub fn init() {
    let cpuid = raw_cpuid::CpuId::new();
    unsafe {
        // Enable the architectural state required by FXSAVE/FXRSTOR. The
        // extended syscall frame uses these instructions on every transition,
        // so do not rely on firmware having configured the control registers.
        assert!(cpuid.get_feature_info().unwrap().has_fxsave_fxstor());
        Cr0::update(|cr0| {
            cr0.remove(Cr0Flags::EMULATE_COPROCESSOR | Cr0Flags::TASK_SWITCHED);
            cr0.insert(Cr0Flags::MONITOR_COPROCESSOR | Cr0Flags::NUMERIC_ERROR);
        });
        Cr4::update(|cr4| {
            cr4.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
        });

        // enable `syscall` instruction
        assert!(
            cpuid
                .get_extended_processor_and_feature_identifiers()
                .unwrap()
                .has_syscall_sysret()
        );
        Efer::update(|efer| {
            efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        });

        // enable `FSGSBASE` instructions
        assert!(cpuid.get_extended_feature_info().unwrap().has_fsgsbase());
        Cr4::update(|cr4| {
            cr4.insert(Cr4Flags::FSGSBASE);
        });

        // flags to clear on syscall
        // copy from Linux 5.0
        // TF|DF|IF|IOPL|AC|NT
        const RFLAGS_MASK: u64 = 0x47700;

        LStar::write(VirtAddr::new(syscall_entry as *const () as usize as u64));
        SFMask::write(RFlags::from_bits(RFLAGS_MASK).unwrap());
    }
}

unsafe extern "sysv64" {
    fn syscall_entry();
    fn syscall_return(regs: &mut UserContext);
    fn syscall_return_extended(regs: &mut UserContextWithExtensions);
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
        unsafe { syscall_return(self) }
    }
}

impl UserContextWithExtensions {
    /// Goes to user space while preserving x87 and SSE state.
    pub fn run(&mut self) {
        unsafe { syscall_return_extended(self) }
    }
}
