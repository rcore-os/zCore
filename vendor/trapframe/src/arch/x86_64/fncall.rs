//! Switch context by function call within the same privilege level.
//!
//! # Assumption
//!
//! This module suppose you are running kernel on Linux or macOS with glibc,
//! and your user program is based on musl libc.
//!
//! Because we will store values in their pthread structure.

use super::UserContext;
use core::arch::global_asm;

extern "sysv64" {
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
}

impl UserContext {
    /// Go to user context by function return, within the same privilege level.
    ///
    /// User program should call `syscall_fn_entry()` to return back.
    /// Trap reason and error code will always be set to 0x100 and 0.
    pub fn run_fncall(&mut self) {
        unsafe {
            syscall_fn_return(self);
        }
        self.trap_num = 0x100;
        self.error_code = 0;
    }
}

// User: (musl)
// - fs:0  (pthread.self)       = user fsbase
// - fs:48 (pthread.canary2)    = kernel fsbase
//
// Kernel: (glibc)
// - fs:0  (pthread.self)       = kernel fsbase
// - fs:64 (pthread.???)        = kernel stack
// - fs:72 (pthread.???)        = init user fsbase
//
#[cfg(target_os = "linux")]
global_asm!(
    r#"
.macro SWITCH_TO_KERNEL_STACK
    mov rsp, fs:48          # rsp = kernel fsbase
    mov rsp, [rsp + 64]     # rsp = kernel stack
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
    mov rsi, fs:48          # rsi = kernel fsbase
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
    mov edi, 0x1002         # SET_FS
    syscall                 # set fsbase
    mov fs:48, rdx          # user_fs:48 = kernel fsbase
.endm

.global syscall_fn_entry
.global syscall_fn_return
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
.set _syscall_fn_entry, syscall_fn_entry
.set _syscall_fn_return, syscall_fn_return
"#
);

global_asm!(
    r#"
syscall_fn_entry:
    # save rsp
    lea r11, [rsp + 8]      # save rsp to r11 (clobber)

    SWITCH_TO_KERNEL_STACK
    pop rsp
    lea rsp, [rsp + 20*8]   # rsp = top of trap frame

    # push trap frame (struct GeneralRegs)
    push 0                  # ignore gs_base
    PUSH_USER_FSBASE
    pushfq                  # push rflags
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
    # save callee-saved registers
    push r15
    push r14
    push r13
    push r12
    push rbp
    push rbx

    push rdi
    SAVE_KERNEL_STACK
    mov rsp, rdi

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
    use core::arch::global_asm;

    #[cfg(target_os = "macos")]
    global_asm!(".set _dump_registers, dump_registers");

    // Mock user program to dump registers at stack.
    global_asm!(
        r#"
dump_registers:
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
        extern "sysv64" {
            fn dump_registers();
        }
        let mut stack = [0u8; 0x1000];
        let mut cx = UserContext {
            general: GeneralRegs {
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
                rip: dump_registers as usize,
                rflags: 0,
                fsbase: 0, // don't set to non-zero garbage value
                gsbase: 0,
            },
            trap_num: 0,
            error_code: 0,
        };
        cx.run_fncall();
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
