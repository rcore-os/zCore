# User: (musl)
# - tp:0  (pthread.self)       = user tp
# - tp:48 (pthread.canary2)    = user context
#
# Kernel: (glibc)
# - tp:0  (pthread.self)       = kernel tp
# - tp:72 (pthread.???)        = init user tp

.global syscall_fn_entry
.global syscall_fn_return

syscall_fn_entry:
    # save 2 registers for scratch
    stp     x0, x30, [sp, #-16] // save x0, x30 at user stack

    # switch to kernel sp
    mrs     x0, tpidr_el0       // x0 = user tp
    ldr     x0, [x0, #48]       // x0 = user context
    mov     x30, sp             // x30 = user stack
    str     x30, [x0, #4 * 8]   // save user stack
    add     sp, x0, #38 * 8     // sp = top of user context

    # recover x0, x30
    ldp     x0, x30, [x30, #-16]

    # save general registers
    stp     x30, x0, [sp, #-16]!
    str     x29, [sp, #-16]!
    stp     x27, x28, [sp, #-16]!
    stp     x25, x26, [sp, #-16]!
    stp     x23, x24, [sp, #-16]!
    stp     x21, x22, [sp, #-16]!
    stp     x19, x20, [sp, #-16]!
    stp     x17, x18, [sp, #-16]!
    stp     x15, x16, [sp, #-16]!
    stp     x13, x14, [sp, #-16]!
    stp     x11, x12, [sp, #-16]!
    stp     x9, x10, [sp, #-16]!
    stp     x7, x8, [sp, #-16]!
    stp     x5, x6, [sp, #-16]!
    stp     x3, x4, [sp, #-16]!
    stp     x1, x2, [sp, #-16]!

    # skip sp and save tpidr
    mrs     x1, tpidr_el0
    str     x1, [sp, #-8]
    add     sp, sp, #-16

    # skip spsr and save elr(lr)
    ldr     x1, [sp, #32*8]
    str     x1, [sp, #-16]!

    # skip trap num and read kernel sp
    ldr     x1, [sp, #-8]
    mov     sp, x1

    # load kernel tp
    ldr     x1, [sp], #16
    msr     tpidr_el0, x1

    # load callee-saved registers
    ldp     x19, x20, [sp], #16
    ldp     x21, x22, [sp], #16
    ldp     x23, x24, [sp], #16
    ldp     x25, x26, [sp], #16
    ldp     x27, x28, [sp], #16
    ldp     x29, x30, [sp], #16

    ret

    # extern "C" fn syscall_fn_return(&mut UserContext)
syscall_fn_return:
    # save callee-saved registers
    stp     x29, x30, [sp, #-16]!
    stp     x27, x28, [sp, #-16]!
    stp     x25, x26, [sp, #-16]!
    stp     x23, x24, [sp, #-16]!
    stp     x21, x22, [sp, #-16]!
    stp     x19, x20, [sp, #-16]!

    # save kernel tp
    mrs     x8, tpidr_el0   // x8 = kernel tp
    str     x8, [sp, #-16]!

    # save kernel sp to UserContext
    mov     x9, sp
    str     x9, [x0, #8]

    # pop tpidr
    ldr     x9, [x0, #5*8]  // x9 = user tp
    cbnz    x9, 1f          // if not 0, goto set
    add     x9, x8, #72     // x9 = init user tp
1:  msr     tpidr_el0, x9   // tp = x9
    str     x0, [x9, #48]   // user_tp:48 = user context

    # pop elr, sp
    ldr     x30, [x0, #2*8]
    ldr     x8, [x0, #4*8]
    mov     sp, x8

    # pop general registers
    add     x0, x0, #6*8
    ldp     x1, x2, [x0], #16
    ldp     x3, x4, [x0], #16
    ldp     x5, x6, [x0], #16
    ldp     x7, x8, [x0], #16
    ldp     x9, x10, [x0], #16
    ldp     x11, x12, [x0], #16
    ldp     x13, x14, [x0], #16
    ldp     x15, x16, [x0], #16
    ldp     x17, x18, [x0], #16
    ldp     x19, x20, [x0], #16
    ldp     x21, x22, [x0], #16
    ldp     x23, x24, [x0], #16
    ldp     x25, x26, [x0], #16
    ldp     x27, x28, [x0], #16
    ldr     x29, [x0], #16
    ldr     x0, [x0, #8]
    ret
