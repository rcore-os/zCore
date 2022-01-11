	.section .text.entry
	.globl _start
_start:
	#关中断
	csrw sie, zero

	#关闭mmu
	#csrw satp, zero
	#BSS节清零
	la t0, sbss
	la t1, ebss
	bgeu t0, t1, secondary_hart_start

clear_bss_loop:
	# sd: store double word (64 bits)
	sd zero, (t0)
	addi t0, t0, 8
	bltu t0, t1, clear_bss_loop

primary_hart:
	call init_vm
	lui t0, %hi(primary_rust_main)
	addi t0, t0, %lo(primary_rust_main)
	jr t0


.globl secondary_hart_start
secondary_hart_start:
	csrw sie, zero
	call init_vm
	lui t0, %hi(secondary_rust_main)
	addi t0, t0, %lo(secondary_rust_main)
	jr t0

init_vm:
	#la sp, bootstacktop
	#call rust_main

	#可清零低12位地址
	lui t0, %hi(boot_page_table_sv39)
	li t1, PHY_MEM_OFS #立即数加载
	#计算出页表的物理地址
	sub t0, t0, t1

	#右移12位，变为satp的PPN
	srli t0, t0, 12

	#satp的MODE设为Sv39
	li t1, 8 << 60

	#写satp
	or t0, t0, t1
	csrw satp, t0

	#刷新TLB
	sfence.vma

	li t0, 4096 * 16
	mul t0, t0, a0
	#此时在虚拟内存空间，设置sp为虚拟地址
	lui sp, %hi(bootstacktop)
	sub sp, sp, t0
	ret

	.section .bss.stack
	.align 12
	.global bootstack
bootstack:
	.space 4096 * 160
	.global bootstacktop
bootstacktop:

