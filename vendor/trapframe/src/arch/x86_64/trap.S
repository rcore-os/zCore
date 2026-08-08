.text
.global __alltraps
__alltraps:
    push rax

    /*
    kernel stack:
    - ptr to GeneralRegs
    - ss
    - rsp
    - rflags
    - cs
    - rip
    - error code
    - trap num
    - rax
    */
    mov ax, [rsp + 4*8]     # load cs
    and ax, 0x3             # test
    jz __from_kernel        # continue trap

__from_user:
    swapgs                  # swap in kernel gs
    mov rax, [rsp + 6*8]    # rax = user rsp
    mov gs:12, rax          # store user rsp -> scratch at TSS.sp1

    mov rsp, [rsp + 8*8]    # load rsp = bottom of trap frame
    add rsp, 22*8           # rsp = top of trap frame
    mov rax, gs:4           # rax = kernel stack

    # push trap_num, error_code
    push [rax - 6*8]        # push error_code
    push [rax - 7*8]        # push trap_num
    push rax                # skip gsbase
    push rax                # skip fsbase
    # push general registers
    push [rax - 3*8]        # push rflags
    push [rax - 5*8]        # push rip
    mov rax, [rax - 8*8]    # pop rax
    jmp trap_syscall_entry

__from_kernel:
    pop rax
    push 0
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    # Long mode always pushes SS:RSP (even same-CPL). After the 9 pushes
    # above (pad + r15..r8), the hardware frame is at:
    #   [rsp+9]=trap_num [+10]=err [+11]=rip [+12]=cs [+13]=rflags
    #   [+14]=saved_RSP [+15]=ss
    # A prior `lea r8,[rsp+13*8]` stored *&rflags* into tf.rsp. Then
    # try_skip_null_execute_call read [tf.rsp] as a "return address" and
    # treated RFLAGS|RF (often 0x13xxx, inside the .text low32 window) as
    # soft-smash residue — false positive that halted desktop bring-up as
    # `[rsp]=0x13446` while hard guards were never touched. Load the CPU-
    # saved RSP *value* so tf.rsp is the faulting stack pointer.
    mov r8, [rsp + 14*8]
    push r8                 # push rsp (faulting RSP value)
    push rbp
    push rdi
    push rsi
    push rdx
    push rcx
    push rbx
    push rax

    mov rdi, rsp
    call trap_handler

.global trap_return
trap_return:
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

    # skip padding, trap_num, error_code
    add rsp, 24

    iretq
