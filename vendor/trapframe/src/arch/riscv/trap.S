# Constants / Macros defined in Rust code:
#   XLENB
#   LOAD
#   STORE

    .section .text
    .global trap_entry
    .balign 4
trap_entry:
    # If coming from userspace, preserve the user stack pointer and load
    # the kernel stack pointer. If we came from the kernel, sscratch
    # will contain 0, and we should continue on the current stack.
    csrrw sp, sscratch, sp
    bnez sp, trap_from_user
trap_from_kernel:
    csrr sp, sscratch
    addi sp, sp, -34 * XLENB
    # sscratch = previous-sp, sp = kernel-sp
trap_from_user:
    # save general registers except sp(x2)
    STORE_SP x1, 1
    STORE_SP x3, 3
    STORE_SP x4, 4
    STORE_SP x5, 5
    STORE_SP x6, 6
    STORE_SP x7, 7
    STORE_SP x8, 8
    STORE_SP x9, 9
    STORE_SP x10, 10
    STORE_SP x11, 11
    STORE_SP x12, 12
    STORE_SP x13, 13
    STORE_SP x14, 14
    STORE_SP x15, 15
    STORE_SP x16, 16
    STORE_SP x17, 17
    STORE_SP x18, 18
    STORE_SP x19, 19
    STORE_SP x20, 20
    STORE_SP x21, 21
    STORE_SP x22, 22
    STORE_SP x23, 23
    STORE_SP x24, 24
    STORE_SP x25, 25
    STORE_SP x26, 26
    STORE_SP x27, 27
    STORE_SP x28, 28
    STORE_SP x29, 29
    STORE_SP x30, 30
    STORE_SP x31, 31

    # save sp, sstatus, sepc
    csrrw t0, sscratch, x0  # sscratch = 0 (kernel)
    csrr t1, sstatus
    csrr t2, sepc
    STORE_SP t0, 2          # save sp
    STORE_SP t1, 32         # save sstatus
    STORE_SP t2, 33         # save sepc

    andi t1, t1, 1 << 8     # sstatus.SPP = 1
    beqz t1, end_trap_from_user
end_trap_from_kernel:
    mv a0, sp               # first arg is TrapFrame
    la ra, trap_return      # set return address
    tail trap_handler

end_trap_from_user:
    .ifdef RISCV_EXTENDED_CONTEXT
    LOAD_SP t3, 0
    andi t3, t3, 1
    beqz t3, 4f
    LOAD_SP t1, 32
    # VS=Off means this hart either has no vector extension or the user did not
    # enable it.  Do not execute vector instructions in either case.
    li t3, 3 << 9
    and t3, t1, t3
    beqz t3, 1f
    .option push
    .option arch, +v, +d
    # UserContext starts with 34 XLEN words.  Save four groups of eight
    # whole vector registers; this feature currently targets VLEN=128.
    addi t0, sp, 34 * XLENB
    vs8r.v v0, (t0)
    addi t0, t0, 128
    vs8r.v v8, (t0)
    addi t0, t0, 128
    vs8r.v v16, (t0)
    addi t0, t0, 128
    vs8r.v v24, (t0)
    csrr t0, vstart
    csrr t1, vl
    csrr t2, vtype
    STORE_SP t0, 98
    STORE_SP t1, 99
    STORE_SP t2, 100
    csrr t0, vcsr
    STORE_SP t0, 101
    .option pop
1:
    # Floating-point state is independent of vector state.
    LOAD_SP t1, 32
    li t3, 3 << 13
    and t3, t1, t3
    beqz t3, 2f
    .option push
    .option arch, +d
    fsd f0, 102 * XLENB(sp)
    fsd f1, 103 * XLENB(sp)
    fsd f2, 104 * XLENB(sp)
    fsd f3, 105 * XLENB(sp)
    fsd f4, 106 * XLENB(sp)
    fsd f5, 107 * XLENB(sp)
    fsd f6, 108 * XLENB(sp)
    fsd f7, 109 * XLENB(sp)
    fsd f8, 110 * XLENB(sp)
    fsd f9, 111 * XLENB(sp)
    fsd f10, 112 * XLENB(sp)
    fsd f11, 113 * XLENB(sp)
    fsd f12, 114 * XLENB(sp)
    fsd f13, 115 * XLENB(sp)
    fsd f14, 116 * XLENB(sp)
    fsd f15, 117 * XLENB(sp)
    fsd f16, 118 * XLENB(sp)
    fsd f17, 119 * XLENB(sp)
    fsd f18, 120 * XLENB(sp)
    fsd f19, 121 * XLENB(sp)
    fsd f20, 122 * XLENB(sp)
    fsd f21, 123 * XLENB(sp)
    fsd f22, 124 * XLENB(sp)
    fsd f23, 125 * XLENB(sp)
    fsd f24, 126 * XLENB(sp)
    fsd f25, 127 * XLENB(sp)
    fsd f26, 128 * XLENB(sp)
    fsd f27, 129 * XLENB(sp)
    fsd f28, 130 * XLENB(sp)
    fsd f29, 131 * XLENB(sp)
    fsd f30, 132 * XLENB(sp)
    fsd f31, 133 * XLENB(sp)
    csrr t0, fcsr
    STORE_SP t0, 134
    .option pop
2:
4:
    .endif
    # load callee-saved registers
    LOAD_SP t0, 0
    andi t0, t0, -2
    STORE_SP t0, 0
    mv sp, t0
    LOAD_SP s0, 0
    LOAD_SP s1, 1
    LOAD_SP s2, 2
    LOAD_SP s3, 3
    LOAD_SP s4, 4
    LOAD_SP s5, 5
    LOAD_SP s6, 6
    LOAD_SP s7, 7
    LOAD_SP s8, 8
    LOAD_SP s9, 9
    LOAD_SP s10, 10
    LOAD_SP s11, 11
    LOAD_SP ra, 12
    # not callee-saved, but is used to store mhartid
    LOAD_SP tp, 13
    addi sp, sp, 14 * XLENB

    ret

.global run_user
.ifdef RISCV_EXTENDED_CONTEXT
.global run_user_extended
.endif
run_user:
    li t1, 0
    j run_user_common
.ifdef RISCV_EXTENDED_CONTEXT
run_user_extended:
    li t1, 1
.endif
run_user_common:
    # save callee-saved registers
    addi sp, sp, -14 * XLENB
    STORE_SP s0, 0
    STORE_SP s1, 1
    STORE_SP s2, 2
    STORE_SP s3, 3
    STORE_SP s4, 4
    STORE_SP s5, 5
    STORE_SP s6, 6
    STORE_SP s7, 7
    STORE_SP s8, 8
    STORE_SP s9, 9
    STORE_SP s10, 10
    STORE_SP s11, 11
    STORE_SP ra, 12
    # not callee-saved, but is used to store mhartid
    STORE_SP tp, 13

    mv t0, sp
    or t0, t0, t1
    mv sp, a0
    STORE_SP t0, 0          # save kernel-sp
    csrw sscratch, sp       # sscratch = bottom of trap frame

trap_return:
    LOAD_SP t0, 32          # t0 = sstatus
    LOAD_SP t1, 33          # t1 = sepc
    .ifdef RISCV_EXTENDED_CONTEXT
    # Kernel traps use the smaller TrapFrame, so only restore extension state
    # when returning to userspace (sstatus.SPP == 0).
    andi t2, t0, 1 << 8
    bnez t2, 3f
    LOAD_SP t2, 0
    andi t2, t2, 1
    beqz t2, 3f
    li t3, 3 << 9
    and t3, t0, t3
    beqz t3, 1f
    .option push
    .option arch, +v, +d
    addi t2, sp, 34 * XLENB
    vl8re8.v v0, (t2)
    addi t2, t2, 128
    vl8re8.v v8, (t2)
    addi t2, t2, 128
    vl8re8.v v16, (t2)
    addi t2, t2, 128
    vl8re8.v v24, (t2)
    LOAD_SP t2, 99
    LOAD_SP t3, 100
    vsetvl zero, t2, t3
    LOAD_SP t2, 101
    csrw vcsr, t2
    LOAD_SP t2, 98
    csrw vstart, t2
    .option pop
1:
    li t3, 3 << 13
    and t3, t0, t3
    beqz t3, 3f
    .option push
    .option arch, +d
    fld f0, 102 * XLENB(sp)
    fld f1, 103 * XLENB(sp)
    fld f2, 104 * XLENB(sp)
    fld f3, 105 * XLENB(sp)
    fld f4, 106 * XLENB(sp)
    fld f5, 107 * XLENB(sp)
    fld f6, 108 * XLENB(sp)
    fld f7, 109 * XLENB(sp)
    fld f8, 110 * XLENB(sp)
    fld f9, 111 * XLENB(sp)
    fld f10, 112 * XLENB(sp)
    fld f11, 113 * XLENB(sp)
    fld f12, 114 * XLENB(sp)
    fld f13, 115 * XLENB(sp)
    fld f14, 116 * XLENB(sp)
    fld f15, 117 * XLENB(sp)
    fld f16, 118 * XLENB(sp)
    fld f17, 119 * XLENB(sp)
    fld f18, 120 * XLENB(sp)
    fld f19, 121 * XLENB(sp)
    fld f20, 122 * XLENB(sp)
    fld f21, 123 * XLENB(sp)
    fld f22, 124 * XLENB(sp)
    fld f23, 125 * XLENB(sp)
    fld f24, 126 * XLENB(sp)
    fld f25, 127 * XLENB(sp)
    fld f26, 128 * XLENB(sp)
    fld f27, 129 * XLENB(sp)
    fld f28, 130 * XLENB(sp)
    fld f29, 131 * XLENB(sp)
    fld f30, 132 * XLENB(sp)
    fld f31, 133 * XLENB(sp)
    LOAD_SP t2, 134
    csrw fcsr, t2
    .option pop
3:
    .endif
    csrw sstatus, t0        # load sstatus
    csrw sepc, t1           # load sepc

    # restore general registers except sp(x2)
    LOAD_SP x1, 1
    LOAD_SP x3, 3
    LOAD_SP x4, 4
    LOAD_SP x5, 5
    LOAD_SP x6, 6
    LOAD_SP x7, 7
    LOAD_SP x8, 8
    LOAD_SP x9, 9
    LOAD_SP x10, 10
    LOAD_SP x11, 11
    LOAD_SP x12, 12
    LOAD_SP x13, 13
    LOAD_SP x14, 14
    LOAD_SP x15, 15
    LOAD_SP x16, 16
    LOAD_SP x17, 17
    LOAD_SP x18, 18
    LOAD_SP x19, 19
    LOAD_SP x20, 20
    LOAD_SP x21, 21
    LOAD_SP x22, 22
    LOAD_SP x23, 23
    LOAD_SP x24, 24
    LOAD_SP x25, 25
    LOAD_SP x26, 26
    LOAD_SP x27, 27
    LOAD_SP x28, 28
    LOAD_SP x29, 29
    LOAD_SP x30, 30
    LOAD_SP x31, 31
    # restore sp last
    LOAD_SP x2, 2

    # return from supervisor call
    sret
