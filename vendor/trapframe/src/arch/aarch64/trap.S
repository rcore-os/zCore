.section .text

.global __alltraps
__alltraps:
    # x30 and x0 are saved in __vectors
    # x0 is trap num now
    # skip __reversed
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

    # skip tpidr and sp
    add     sp, sp, #-16

    # read spsr and elr
    mrs     x2, spsr_el1
    mrs     x1, elr_el1
    stp     x1, x2, [sp, #-16]!

    # save trap num
    str     x0, [sp, #-16]!

    # check source is 2
    mov     x1, #0x3
    and     x1, x1, x0
    cmp     x1, #2
    beq     trap_from_user

trap_from_kernel:
    # read tpidr and sp
    mrs     x2, tpidr_el1
    add     x1, sp, #38*8
    stp     x1, x2, [sp, #32]
    # go to rust
    mov     x0, sp
    bl      trap_handler
    # load tpidr
    ldr     x1, [sp, #40]
    msr     tpidr_el1, x1
    # go to trap_return
    b       trap_return_without_fp

trap_from_user:
    ldr     x3, [sp, #8]
    tbz     x3, #0, trap_from_user_without_fp
    # save floating-point and Advanced SIMD state after the integer frame
    stp     q0, q1, [sp, #304]
    stp     q2, q3, [sp, #336]
    stp     q4, q5, [sp, #368]
    stp     q6, q7, [sp, #400]
    stp     q8, q9, [sp, #432]
    stp     q10, q11, [sp, #464]
    stp     q12, q13, [sp, #496]
    stp     q14, q15, [sp, #528]
    stp     q16, q17, [sp, #560]
    stp     q18, q19, [sp, #592]
    stp     q20, q21, [sp, #624]
    stp     q22, q23, [sp, #656]
    stp     q24, q25, [sp, #688]
    stp     q26, q27, [sp, #720]
    stp     q28, q29, [sp, #752]
    stp     q30, q31, [sp, #784]
    mrs     x3, fpcr
    mrs     x4, fpsr
    add     x5, sp, #816
    stp     w3, w4, [x5]

trap_from_user_without_fp:
    # read tpidr and sp
    mrs     x2, tpidr_el0
    mrs     x1, sp_el0
    stp     x1, x2, [sp, #32]
    # read sp
    ldr     x2, [sp, #8]
    bic     x2, x2, #1
    str     x2, [sp, #8]
    mov     sp, x2

    # load callee-saved registers
    ldp     x19, x20, [sp], #16
    ldp     x21, x22, [sp], #16
    ldp     x23, x24, [sp], #16
    ldp     x25, x26, [sp], #16
    ldp     x27, x28, [sp], #16
    ldp     x29, x30, [sp], #16

    ret

.macro HANDLER source kind
    .align 7
    # sp is set to SP_EL1 upon trap
    stp     lr, x0, [sp, #-16]!
    mov     x0, #\source
    movk    x0, #\kind, lsl #16
    b       __alltraps
.endm

.global __vectors
.align 11
__vectors:
    HANDLER 0 0
    HANDLER 0 1
    HANDLER 0 2
    HANDLER 0 3
    HANDLER 1 0
    HANDLER 1 1
    HANDLER 1 2
    HANDLER 1 3
    HANDLER 2 0
    HANDLER 2 1
    HANDLER 2 2
    HANDLER 2 3
    HANDLER 3 0
    HANDLER 3 1
    HANDLER 3 2
    HANDLER 3 3

.global run_user
.global run_user_extended
run_user:
    mov     x3, #0
    b       run_user_common
run_user_extended:
    mov     x3, #1
run_user_common:
    # x0 points to TrapFrame
    # save callee-saved registers x19-x29
    stp     x29, x30, [sp, #-16]!
    stp     x27, x28, [sp, #-16]!
    stp     x25, x26, [sp, #-16]!
    stp     x23, x24, [sp, #-16]!
    stp     x21, x22, [sp, #-16]!
    stp     x19, x20, [sp, #-16]!

    # save kernel sp to TrapFrame
    mov     x1, sp
    orr     x1, x1, x3
    mov     sp, x0
    str     x1, [sp, #8]

    # load sp and tpidr
    ldp     x1, x2, [sp, #32]
    msr     sp_el0, x1
    msr     tpidr_el0, x2

trap_return:
    # sp points to TrapFrame
    ldr     x3, [sp, #8]
    tbz     x3, #0, trap_return_without_fp
    ldp     q0, q1, [sp, #304]
    ldp     q2, q3, [sp, #336]
    ldp     q4, q5, [sp, #368]
    ldp     q6, q7, [sp, #400]
    ldp     q8, q9, [sp, #432]
    ldp     q10, q11, [sp, #464]
    ldp     q12, q13, [sp, #496]
    ldp     q14, q15, [sp, #528]
    ldp     q16, q17, [sp, #560]
    ldp     q18, q19, [sp, #592]
    ldp     q20, q21, [sp, #624]
    ldp     q22, q23, [sp, #656]
    ldp     q24, q25, [sp, #688]
    ldp     q26, q27, [sp, #720]
    ldp     q28, q29, [sp, #752]
    ldp     q30, q31, [sp, #784]
    add     x3, sp, #816
    ldp     w1, w2, [x3]
    msr     fpcr, x1
    msr     fpsr, x2

trap_return_without_fp:
    # skip trap num, don't restore
    add     sp, sp, #16

    # elr and spsr
    ldp     x1, x2, [sp], #16
    msr     elr_el1, x1
    msr     spsr_el1, x2

    # skip sp and tpidr
    add     sp, sp, #16

    # general purpose registers
    ldp     x1, x2, [sp], #16
    ldp     x3, x4, [sp], #16
    ldp     x5, x6, [sp], #16
    ldp     x7, x8, [sp], #16
    ldp     x9, x10, [sp], #16
    ldp     x11, x12, [sp], #16
    ldp     x13, x14, [sp], #16
    ldp     x15, x16, [sp], #16
    ldp     x17, x18, [sp], #16
    ldp     x19, x20, [sp], #16
    ldp     x21, x22, [sp], #16
    ldp     x23, x24, [sp], #16
    ldp     x25, x26, [sp], #16
    ldp     x27, x28, [sp], #16
    ldr     x29, [sp], #16
    ldp     lr, x0, [sp], #16

    # return
    eret
