# Context switch
#
#   void swtch(struct ContextData *old, struct ContextData *new);
#
# Save current registers in old. Load from new.

    .text
    .globl switch
    .balign 4
switch:
        add     x0, x0, #112
        mrs     x2, ttbr0_el1
        mov     x3, sp
        stp     x3, x2, [x0, #-16]!
        stp     x29, x30, [x0, #-16]!
        stp     x27, x28, [x0, #-16]!
        stp     x25, x26, [x0, #-16]!
        stp     x23, x24, [x0, #-16]!
        stp     x21, x22, [x0, #-16]!
        stp     x19, x20, [x0, #-16]!

        ldr     x9, [x1, #104]
        lsr     x10, x9, #12
        msr     ttbr0_el1, x9
        dsb     ishst
        tlbi    vaae1is, x10
        dsb     ish
        isb

        ldp     x19, x20, [x1], #16
        ldp     x21, x22, [x1], #16
        ldp     x23, x24, [x1], #16
        ldp     x25, x26, [x1], #16
        ldp     x27, x28, [x1], #16
        ldp     x29, x30, [x1], #16
        ldr     x0, [x1], #8
        mov     sp, x0
        ret