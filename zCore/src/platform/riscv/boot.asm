.equ STACK_MAX, 4096 * 16
.equ STACK_MAX_HARTS, 8

	.section .text.entry
	.globl _start
_start:
	#关中断
	csrw sie, zero
	csrw sip, zero

	#关闭mmu
	#csrw satp, zero

	#BSS节清零
	la t0, sbss
	la t1, ebss
	bgeu t0, t1, primary_hart

clear_bss_loop:
	# sd: store double word (64 bits)
	sd zero, (t0)
	addi t0, t0, 8
	bltu t0, t1, clear_bss_loop
	
primary_hart:
	call init_vm
	la t0, primary_rust_main
	la t1, PHY_MEM_OFS
	ld t1, (t1)
	add t0, t0, t1
	jr t0

.globl secondary_hart_start
secondary_hart_start:
	csrw sie, zero
	csrw sip, zero
	call init_vm
	la t0, secondary_rust_main
	la t1, PHY_MEM_OFS
	ld t1, (t1)
	add t0, t0, t1
	jr t0

init_vm:
	#获取页表的物理地址
	la t0, boot_page_table_sv39

	#右移12位，变为satp的PPN
	srli t0, t0, 12

	#satp的MODE设为Sv39
	li t1, 8 << 60

	#写satp
	or t0, t0, t1

	#刷新TLB
	sfence.vma

	csrw satp, t0

	#此时在虚拟内存空间，设置sp为虚拟地址
	li t0, STACK_MAX
	mul t0, t0, a0

	la t1, boot_stack_top
	la t2, PHY_MEM_OFS
	ld t2, (t2)
	add sp, t1, t2

	#计算多个核的sp偏移
	sub sp, sp, t0
	ret

	.section .data
	.align 12 #12位对齐
boot_page_table_sv39:
	#1G的一个大页: 0x00000000_00000000 --> 0x00000000
	#1G的一个大页: 0x00000000_80000000 --> 0x80000000
	#1G的一个大页: 0xffffffe0_00000000 --> 0x00000000
	#1G的一个大页: 0xffffffe0_80000000 --> 0x80000000

	.quad (0 << 10) | 0xef
	.zero 8
	.quad (0x80000 << 10) | 0xef

	.zero 8 * 381
	.quad (0 << 10) | 0xef
	.zero 8
	.quad (0x80000 << 10) | 0xef
	.zero 8 * 125

	.section .bss.stack
	.align 12
	.global boot_stack
boot_stack:
	.space STACK_MAX * STACK_MAX_HARTS
	.global boot_stack_top
boot_stack_top:
