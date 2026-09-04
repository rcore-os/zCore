//! Switch context by function call within the same privilege level.
//!
//! # Assumption
//!
//! This module suppose you are running kernel on Linux or macOS with glibc,
//! and your user program is based on musl libc.
//!
//! Because we will store values in their pthread structure.

use super::{UserContext, UserContextWithExtensions};
use core::arch::global_asm;

unsafe extern "sysv64" {
    /// The syscall entry of function call.
    ///
    /// # Usage
    ///
    /// Replace `syscall` instruction by a `call` instruction.
    ///
    /// ```asm
    /// syscall
    /// call syscall_fn_entry
    /// ```
    pub fn syscall_fn_entry();

    fn syscall_fn_return(regs: &mut UserContext);
    fn syscall_fn_return_extended(regs: &mut UserContextWithExtensions);
}

impl UserContext {
    /// Go to user context by function return, within the same privilege level.
    ///
    /// User program should call `syscall_fn_entry()` to return back.
    /// Trap reason and error code will always be set to 0x100 and 0.
    pub fn run_fncall(&mut self) {
        unsafe { syscall_fn_return(self) }
        self.trap_num = 0x100;
        self.error_code = 0;
    }
}

impl UserContextWithExtensions {
    /// Goes to user context while preserving x87 and SSE state.
    pub fn run_fncall(&mut self) {
        unsafe { syscall_fn_return_extended(self) }
        self.trap_num = 0x100;
        self.error_code = 0;
    }
}

// User: (musl)
// - fs:0                       = user fsbase
// - gs:0                       = kernel fsbase
// - gs:64 (pthread.???)        = kernel stack
//
// Kernel: (glibc)
// - fs:0                       = kernel fsbase
// - fs:64 (pthread.???)        = kernel stack
// - fs:72 (pthread.???)        = init user fsbase
//
// Older versions stored the kernel FS base at user fs:48.  That location is
// now the Fuchsia stack guard, so libc overwrites it during startup.  Linux
// leaves the userspace GS base available; keep the host TCB there instead.
//
#[cfg(target_os = "linux")]
global_asm!(
    r#"
.macro SWITCH_TO_KERNEL_STACK
    mov rsp, gs:64          # rsp = kernel stack
.endm
.macro SAVE_KERNEL_STACK
    mov fs:64, rsp
.endm
.macro PUSH_USER_FSBASE
    push fs:0
.endm
.macro SWITCH_TO_KERNEL_FSBASE
    mov eax, 158            # SYS_arch_prctl
    mov edi, 0x1002         # SET_FS
    mov rsi, gs:0           # rsi = kernel fsbase
    syscall
.endm
.macro POP_USER_FSBASE
    mov rsi, [rsp + 18 * 8] # rsi = user fsbase
    mov rdx, fs:0           # rdx = kernel fsbase
    test rsi, rsi
    jnz 1f                  # if not 0, goto set
0:  lea rsi, [rdx + 72]     # rsi = init user fsbase
    mov [rsi], rsi          # user_fs:0 = user fsbase
1:  mov eax, 158            # SYS_arch_prctl
    mov edi, 0x1001         # SET_GS
    mov rsi, rdx            # rsi = kernel fsbase
    syscall                 # keep kernel fsbase at gs:0 while in user code
    mov rsi, [rsp + 18 * 8] # reload user fsbase (syscall clobbers r11)
    test rsi, rsi
    jnz 2f
    lea rsi, [rdx + 72]
2:  mov eax, 158            # SYS_arch_prctl
    mov edi, 0x1002         # SET_FS
    syscall                 # set fsbase
.endm

.global syscall_fn_entry
.global syscall_fn_return
.global syscall_fn_return_extended
"#
);

// User: (musl)
// - gs:0   (pthread.self)      = user gsbase
// - gs:48  (pthread.canary2)   = kernel gsbase
//
// Kernel: (darwin)
// - gs:0   (pthread.tsd[self]) = kernel gsbase - 224
// - gs:48  (pthread.tsd[6])    = kernel stack
// - gs:240 (pthread.tsd[30])   = init user fsbase
//
// Ref:
// - Set gsbase:
//   - https://gist.github.com/aras-p/5389747
// - Get gsbase:
//   - https://github.com/DynamoRIO/dynamorio/issues/1568#issuecomment-239819506
//   - https://github.com/apple/darwin-libpthread/blob/03c4628c8940cca6fd6a82957f683af804f62e7f/src/internal.h#L241
#[cfg(target_os = "macos")]
global_asm!(
    r#"
.macro SWITCH_TO_KERNEL_STACK
    mov rsp, gs:48          # rsp = kernel gsbase
    mov rsp, [rsp + 48]     # rsp = kernel stack
.endm
.macro SAVE_KERNEL_STACK
    mov gs:48, rsp
.endm
.macro PUSH_USER_FSBASE
    push gs:0
.endm
.macro SWITCH_TO_KERNEL_FSBASE
    mov rdi, gs:48          # rdi = kernel gsbase
    mov eax, 0x3000003
    syscall                 # set gsbase
.endm
.macro POP_USER_FSBASE
    mov rdi, [rsp + 18 * 8] # rdi = user gsbase
    mov rsi, gs:0
    add rsi, 224            # rsi = kernel gsbase
    test rdi, rdi
    jnz 1f                  # if not 0, goto set
0:  lea rdi, [rsi + 30*8]   # rdi = init user gsbase
                            #     = pthread.tsd[30] (kernel gsbase + 30 * 8)
    mov [rdi], rdi          # user_gs:0 = user gsbase
1:  mov eax, 0x3000003
    syscall                 # set gsbase
    mov gs:48, rsi          # user_gs:48 = kernel gsbase
.endm

.global _syscall_fn_entry
.global syscall_fn_entry
.global _syscall_fn_return
.global _syscall_fn_return_extended
.set _syscall_fn_entry, syscall_fn_entry
.set _syscall_fn_return, syscall_fn_return
.set _syscall_fn_return_extended, syscall_fn_return_extended
"#
);

global_asm!(
    r#"
syscall_fn_entry:
    pushfq                  # preserve flags before inspecting the tagged context
    # save rsp
    lea r11, [rsp + 16]     # save rsp to r11 (clobber)

    SWITCH_TO_KERNEL_STACK
    pop rsp
    test rsp, 1
    jnz 1f
    and rsp, -2
    mov qword ptr [rsp + 21*8], 0
    jmp 2f
1:  and rsp, -2
    mov qword ptr [rsp + 21*8], 1
2:
    lea rsp, [rsp + 20*8]   # rsp = top of trap frame

    # push trap frame (struct GeneralRegs)
    push 0                  # ignore gs_base
    PUSH_USER_FSBASE
    push [r11 - 16]         # push saved rflags
    push [r11 - 8]          # push rip
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push r11                # push rsp
    push rbp
    push rdi
    push rsi
    push rdx
    push rcx
    push rbx
    push rax

    cmp qword ptr [rsp + 21*8], 0
    je 1f
    fxsave64 [rsp + 22*8]
1:

    # restore callee-saved registers
    SWITCH_TO_KERNEL_STACK
    pop rbx
    pop rbx
    pop rbp
    pop r12
    pop r13
    pop r14
    pop r15

    SWITCH_TO_KERNEL_FSBASE

    # go back to Rust
    ret

    # extern "sysv64" fn syscall_fn_return(&mut UserContext)
syscall_fn_return:
    xor esi, esi
    jmp syscall_fn_return_common
syscall_fn_return_extended:
    mov esi, 1
syscall_fn_return_common:
    # save callee-saved registers
    push r15
    push r14
    push r13
    push r12
    push rbp
    push rbx

    or rdi, rsi
    push rdi
    SAVE_KERNEL_STACK
    and rdi, -2
    mov rsp, rdi

    test esi, esi
    jz 1f
    fxrstor64 [rsp + 22*8]
1:

    POP_USER_FSBASE

    # pop trap frame (struct GeneralRegs)
    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    pop rbp
    pop r8                  # skip rsp
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
    pop r11                 # r11 = rip. FIXME: don't overwrite r11!
    popfq                   # pop rflags
    mov rsp, [rsp - 8*11]   # restore rsp
    jmp r11                 # restore rip
"#
);

#[cfg(test)]
mod tests {
    use crate::*;
    use core::arch::{asm, global_asm};

    #[cfg(target_os = "macos")]
    global_asm!(
        ".set _dump_registers, dump_registers\n\
         .set RESTORED_XMM0, _RESTORED_XMM0\n\
         .set UPDATED_XMM0, _UPDATED_XMM0"
    );

    #[unsafe(no_mangle)]
    static mut RESTORED_XMM0: [u8; 16] = [0; 16];
    #[unsafe(no_mangle)]
    static UPDATED_XMM0: [u8; 16] = [0xa5; 16];

    // Mock user program to dump registers at stack.
    global_asm!(
        r#"
dump_registers:
    movdqu [rip + RESTORED_XMM0], xmm0
    movdqu xmm0, [rip + UPDATED_XMM0]
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rsp
    push rbp
    push rdi
    push rsi
    push rdx
    push rcx
    push rbx
    push rax

    add rax, 10
    add rbx, 10
    add rcx, 10
    add rdx, 10
    add rsi, 10
    add rdi, 10
    add rbp, 10
    add r8, 10
    add r9, 10
    add r10, 10
    add r11, 10
    add r12, 10
    add r13, 10
    add r14, 10
    add r15, 10

    call syscall_fn_entry
"#
    );

    #[test]
    fn run_fncall() {
        unsafe extern "sysv64" {
            fn dump_registers();
        }
        let mut stack = [0u8; 0x1000];
        let general = GeneralRegs {
            rax: 0,
            rbx: 1,
            rcx: 2,
            rdx: 3,
            rsi: 4,
            rdi: 5,
            rbp: 6,
            rsp: stack.as_mut_ptr() as usize + 0x1000,
            r8: 8,
            r9: 9,
            r10: 10,
            r11: 11,
            r12: 12,
            r13: 13,
            r14: 14,
            r15: 15,
            rip: dump_registers as *const () as usize,
            rflags: 0,
            fsbase: 0, // don't set to non-zero garbage value
            gsbase: 0,
        };
        #[repr(C)]
        struct GuardedContext {
            context: UserContext,
            guard: [u8; 512],
        }
        let mut legacy = GuardedContext {
            context: UserContext {
                general,
                ..Default::default()
            },
            guard: [0xa5; 512],
        };
        let mut legacy_xmm0 = [0; 16];
        legacy.context.run_fncall();
        unsafe {
            asm!(
                "movdqu [{buffer}], xmm0",
                buffer = in(reg) legacy_xmm0.as_mut_ptr(),
                options(nostack)
            );
        }
        assert_eq!(legacy.context.trap_num, 0x100);
        assert_eq!(legacy.guard, [0xa5; 512]);
        assert_eq!(legacy_xmm0, UPDATED_XMM0);

        let mut cx = UserContextWithExtensions {
            general,
            trap_num: 0,
            error_code: 0,
            fp_simd: Default::default(),
        };
        cx.fp_simd.bytes[160..176].fill(0x5a);
        cx.run_fncall();
        let restored_xmm0 = unsafe { core::ptr::addr_of!(RESTORED_XMM0).read_volatile() };
        assert_eq!(restored_xmm0, [0x5a; 16]);
        assert_eq!(&cx.fp_simd.bytes[160..176], &UPDATED_XMM0);
        // check restored registers
        let general = unsafe { *(cx.general.rsp as *const GeneralRegs) };
        assert_eq!(
            general,
            GeneralRegs {
                rax: 0,
                rbx: 1,
                rcx: 2,
                rdx: 3,
                rsi: 4,
                rdi: 5,
                rbp: 6,
                // skip rsp
                r8: 8,
                r9: 9,
                r10: 10,
                // skip r11
                r12: 12,
                r13: 13,
                r14: 14,
                r15: 15,
                ..general
            }
        );
        // check saved registers
        assert_eq!(
            cx.general,
            GeneralRegs {
                rax: 10,
                rbx: 11,
                rcx: 12,
                rdx: 13,
                rsi: 14,
                rdi: 15,
                rbp: 16,
                // skip rsp
                r8: 18,
                r9: 19,
                r10: 20,
                // skip r11
                r12: 22,
                r13: 23,
                r14: 24,
                r15: 25,
                ..cx.general
            }
        );
        assert_eq!(cx.trap_num, 0x100);
        assert_eq!(cx.error_code, 0);
    }
}
