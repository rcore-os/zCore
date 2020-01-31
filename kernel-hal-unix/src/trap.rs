use super::GeneralRegs;

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

.macro CALL_HANDLE_SYSCALL
    call handle_syscall
.endm
.global syscall_entry
.global syscall_return
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

.macro CALL_HANDLE_SYSCALL
    call _handle_syscall
.endm
.global _syscall_entry
.global _syscall_return
.set _syscall_entry, syscall_entry
.set _syscall_return, syscall_return
"#
);

global_asm!(
    r#"
.intel_syntax noprefix
syscall_entry:
    # save rsp
    lea r11, [rsp + 8]      # save rsp to r11 (clobber)

    SWITCH_TO_KERNEL_STACK

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

    SWITCH_TO_KERNEL_FSBASE

    # go to Rust
    mov rdi, rsp            # arg0 = *mut GeneralRegs
    CALL_HANDLE_SYSCALL
    jmp .syscall_return_sp

    # extern "C" fn syscall_return(&mut GeneralRegs)
syscall_return:
    push rcx                # stack align to 16B
    SAVE_KERNEL_STACK
    mov rsp, rdi
.syscall_return_sp:
    POP_USER_FSBASE

    # pop trap frame (struct GeneralRegs)
    pop rax
    pop rbx
    pop rcx                 # rcx is clobber
    pop rdx
    pop rsi
    pop rdi
    pop rbp
    pop rcx                 # rcx = rsp
    pop r8
    pop r9
    pop r10
    pop r11                 # r11 is clobber
    pop r12
    pop r13
    pop r14
    pop r15
    pop r11                 # r11 = rip
    popfq                   # pop rflags
    mov rsp, rcx            # restore rsp
    jmp r11                 # restore rip
"#
);

extern "C" {
    pub fn syscall_entry();
    pub fn syscall_return(regs: &mut GeneralRegs) -> !;
}

#[linkage = "weak"]
#[no_mangle]
extern "C" fn handle_syscall(_regs: *mut GeneralRegs) {
    unimplemented!()
}
