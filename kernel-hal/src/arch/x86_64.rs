#[repr(C)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct GeneralRegs {
    pub rax: usize,
    pub rbx: usize,
    pub rcx: usize,
    pub rdx: usize,
    pub rsi: usize,
    pub rdi: usize,
    pub rbp: usize,
    pub rsp: usize,
    pub r8: usize,
    pub r9: usize,
    pub r10: usize,
    pub r11: usize,
    pub r12: usize,
    pub r13: usize,
    pub r14: usize,
    pub r15: usize,
    pub rip: usize,
    pub rflags: usize,
    pub fs_base: usize,
    pub gs_base: usize,
}

impl GeneralRegs {
    pub fn new_fn(entry: usize, sp: usize, arg1: usize, arg2: usize) -> Self {
        GeneralRegs {
            rip: entry,
            rsp: sp,
            rdi: arg1,
            rsi: arg2,
            ..Default::default()
        }
    }

    pub fn clone(&self, newsp: usize, newtls: usize) -> Self {
        GeneralRegs {
            rax: 0,
            rsp: newsp,
            fs_base: newtls,
            ..*self
        }
    }

    pub fn fork(&self) -> Self {
        GeneralRegs { rax: 0, ..*self }
    }

    pub const fn zero() -> Self {
        GeneralRegs {
            rax: 0usize,
            rbx: 0usize,
            rcx: 0usize,
            rdx: 0usize,
            rsi: 0usize,
            rdi: 0usize,
            rbp: 0usize,
            rsp: 0usize,
            r8: 0usize,
            r9: 0usize,
            r10: 0usize,
            r11: 0usize,
            r12: 0usize,
            r13: 0usize,
            r14: 0usize,
            r15: 0usize,
            rip: 0usize,
            rflags: 0usize,
            fs_base: 0usize,
            gs_base: 0usize,
        }
    }
}
