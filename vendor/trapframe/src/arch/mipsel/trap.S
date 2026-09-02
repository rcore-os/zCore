    .set noat
    .set noreorder
    .section .text
    .balign 4096
    .global trap_entry

trap_entry:
    b general_trap_vec
    nop

.org 0x180
general_trap_vec:
    # save stack sp to k1
    move $k1, $sp
    # read cp0.status
    mfc0 $k0, $12
    # cp0.status.ksu
    andi $k0, $k0, 0x10
    beq $k0, $zero, trap_from_kernel
    nop

trap_from_user:
    # load kstack
    la $k0, _cur_kstack_ptr
    lw $sp, 0($k0)

trap_from_kernel:
    # save general regs
    addiu $sp, $sp, -33*4
    sw $ra, 128($sp)
    sw $fp, 124($sp)
    sw $k1, 120($sp)  # k1 = old $sp
    sw $gp, 116($sp)
    sw $k1, 112($sp)  # real k1 is damaged
    sw $k0, 108($sp)  # real k0 is damaged
    sw $t9, 104($sp)
    sw $t8, 100($sp)
    sw $s7, 96($sp)
    sw $s6, 92($sp)
    sw $s5, 88($sp)
    sw $s4, 84($sp)
    sw $s3, 80($sp)
    sw $s2, 76($sp)
    sw $s1, 72($sp)
    sw $s0, 68($sp)
    sw $t7, 64($sp)
    sw $t6, 60($sp)
    sw $t5, 56($sp)
    sw $t4, 52($sp)
    sw $t3, 48($sp)
    sw $t2, 44($sp)
    sw $t1, 40($sp)
    sw $t0, 36($sp)
    sw $a3, 32($sp)
    sw $a2, 28($sp)
    sw $a1, 24($sp)
    sw $a0, 20($sp)
    sw $v1, 16($sp)
    sw $v0, 12($sp)
    sw $AT, 8($sp)
    mflo $t1
	sw $t1, 4($sp)
	mfhi $t0
	sw $t0, 0($sp)

    # save special registers
    addiu $sp, $sp, -6*4
    mfc0 $t0, $8     # cp0.vaddr
	sw $t0, 20($sp)
    mfc0 $t0, $14     # cp0.epc
	sw $t0, 16($sp)
    mfc0 $t0, $13     # cp0.cause
	sw $t0, 12($sp)
    mfc0 $t0, $12     # cp0.status
	sw $t0, 8($sp)
    # no need to save tls

    # read cp0.status
    mfc0 $k0, $12
    # mask cp0.status.ksu, status.exl and status.ie
    li $t0, ~0x1b
    and $t1, $k0, $t0
    # write cp0.status
    mtc0 $t1, $12

    # check cp0.status.ksu
    andi $k0, $k0, 0x10
    beq $k0, $zero, end_trap_from_kernel
    nop

end_trap_from_user:
    # read kernel sp
    lw $t1, 4($sp)

    # load callee-saved registers
    move $a0, $sp
    move $sp, $t1
    lw $gp, 40($sp)
    lw $fp, 36($sp)
    lw $s7, 32($sp)
    lw $s6, 28($sp)
    lw $s5, 24($sp)
    lw $s4, 20($sp)
    lw $s3, 16($sp)
    lw $s2, 12($sp)
    lw $s1, 8($sp)
    lw $s0, 4($sp)
    lw $ra, 0($sp)
    addiu $sp, $sp, 4*11

    jr $ra
    nop

end_trap_from_kernel:
    # first arg
    move $a0, $sp
    la $ra, trap_return
    j trap_handler
    nop

    .global run_user
run_user:
    # $a0 points to UserContext
    # save callee-saved registers
    addiu $sp, $sp, -4*11
    sw $gp, 40($sp)
    sw $fp, 36($sp)
    sw $s7, 32($sp)
    sw $s6, 28($sp)
    sw $s5, 24($sp)
    sw $s4, 20($sp)
    sw $s3, 16($sp)
    sw $s2, 12($sp)
    sw $s1, 8($sp)
    sw $s0, 4($sp)
    sw $ra, 0($sp)

    move $t1, $sp
    move $sp, $a0
    sw $t1, 4($sp)

    .global trap_return
trap_return:
    # sp points to TrapFrame
    # restore special registers
	lw   $t1, 8($sp)
	ori  $t1, $t1, 0x2  # status.exl
	mtc0 $t1, $12      # cp0.status

    lw $k0, 16($sp)
	mtc0 $k0, $14      # cp0.epc

    # restore general regs
    lw $t0, 24($sp)
	mthi $t0
	lw $t1, 28($sp)
	mtlo $t1
    lw $at, 32($sp)
	lw $v0, 36($sp)
	lw $v1, 40($sp)
	lw $a0, 44($sp)
	lw $a1, 48($sp)
	lw $a2, 52($sp)
	lw $a3, 56($sp)
	lw $t0, 60($sp)
	lw $t1, 64($sp)
	lw $t2, 68($sp)
	lw $t3, 72($sp)
	lw $t4, 76($sp)
	lw $t5, 80($sp)
	lw $t6, 84($sp)
	lw $t7, 88($sp)
	lw $s0, 92($sp)
	lw $s1, 96($sp)
	lw $s2, 100($sp)
	lw $s3, 104($sp)
	lw $s4, 108($sp)
	lw $s5, 112($sp)
	lw $s6, 116($sp)
	lw $s7, 120($sp)
	lw $t8, 124($sp)
	lw $t9, 128($sp)
	# lw $k0, 132($sp)
	# lw $k1, 136($sp)
	lw $gp, 140($sp)
	# lw $sp, 144($sp)
	lw $fp, 148($sp)
	lw $ra, 152($sp)

    # save kernel stack
    addiu $k1, $sp, 4*39
    la $k0, _cur_kstack_ptr
    sw $k1, 0($k0)

	lw $sp, 144($sp)
    eret

    .section .bss

    # only one cpu is supported
    .global _cur_kstack_ptr
_cur_kstack_ptr:
    .space 4 # 4bytes