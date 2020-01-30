use super::{switch_to_kernel, switch_to_user, GeneralRegs};

#[cfg(target_os = "linux")]
global_asm!(
    r#"
.macro SWITCH_TO_KERNEL_STACK
    mov rsp, gs:64
.endm
.macro CALL_RUST_SYSCALL_ENTRY
    call rust_syscall_entry
.endm
.global syscall_entry
.global syscall_return
"#
);

#[cfg(target_os = "macos")]
global_asm!(
    r#"
.macro SWITCH_TO_KERNEL_STACK
    mov rsp, gs:48          # rsp = kernel gsbase
    mov rsp, [rsp - 48]     # rsp = kernel stack
.endm
.macro CALL_RUST_SYSCALL_ENTRY
    call _rust_syscall_entry
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
    # save rip & rsp
    pop rcx                 # save rip to rcx (clobber)
    mov r11, rsp            # save rsp to r11 (clobber)

    SWITCH_TO_KERNEL_STACK

    # push trap frame (struct GeneralRegs)
    push 0                  # ignore gs_base
    push 0                  # ignore fs_base
    pushfq                  # push rflags
    push rcx                # push rip
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

    # go to Rust
    mov rdi, rsp            # arg0 = *mut GeneralRegs
    CALL_RUST_SYSCALL_ENTRY
    jmp syscall_return_sp

syscall_return:
    mov rsp, rdi
    jmp syscall_return_sp
syscall_return_sp:
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
    // defined by user
    fn handle_syscall(regs: *mut GeneralRegs);
    pub fn syscall_return(regs: &GeneralRegs) -> !;
}

#[no_mangle]
pub unsafe extern "C" fn rust_syscall_entry(regs: *mut GeneralRegs) {
    switch_to_kernel();
    handle_syscall(regs);
    switch_to_user();
}
