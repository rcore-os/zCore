# Context switch
#
#   void swtch(struct ContextData *old, struct ContextData *new);
#
# Save current registers in old. Load from new.

    .text
    .globl switch
    .balign 4
switch:
        // push rip by caller

        // Save callee-save registers
        push rbx
        push rbp
        push r12
        push r13
        push r14
        push r15
        mov r15, cr3
        push r15

        mov [rdi], rsp      // rdi = from_rsp
        mov rsp, [rsi]      // rsi = to_rsp

        // [null-exec guard, last-instant] The pre-switch validation
        // (executor_frame_resumable) reads [sp+0x38] under the runtime lock,
        // but every surviving capture shows that exact slot popping ZERO here
        // moments later: the zeroing lands in the validation->ret window, or
        // the frame is being consumed twice. Re-check the two
        // instantly-fatal slots with the frame's own rsp, zero instructions
        // away from the pops: [rsp+0x38] == 0 would `ret` to vaddr 0
        // (null-exec) and [rsp] == 0 would load CR3=0 (triple fault). On
        // either, bounce back into the from-context we saved two instructions
        // ago ([rdi] is still that cell) so the caller sees the switch
        // "return" immediately; it re-validates, reports, and retires the
        // executor instead of the machine dying. Counters let the Rust side
        // distinguish this last-instant catch from a parked-long-ago kill.
        cmp qword ptr [rsp + 0x38], 0
        je 2f
        cmp qword ptr [rsp], 0
        je 2f

        // Pop callee-save registers
        pop r15
        mov cr3, r15
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbp
        pop rbx
        // pop rip
        ret

2:      // Dead target frame: record it and bounce back through the
        // just-saved from-frame. rax/rdx are caller-saved — free to clobber.
        mov rdx, rsp                      // dead frame's sp, for the report
        mov rsp, [rdi]                    // our own save from above: always valid
        lea rax, [rip + SWITCH_BOUNCE_SP]
        mov [rax], rdx
        lea rax, [rip + SWITCH_BOUNCE_COUNT]
        lock add qword ptr [rax], 1
        pop r15
        mov cr3, r15
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbp
        pop rbx
        ret
