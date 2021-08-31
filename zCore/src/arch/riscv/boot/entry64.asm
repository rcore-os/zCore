	.section .text.entry
	.globl _start
_start:
	#关中断
	csrw sie, zero

	#关闭mmu
	#csrw satp, zero

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


	#此时在虚拟内存空间，设置sp为虚拟地址
	lui sp, %hi(bootstacktop)
	lui t0, %hi(rust_main)
	addi t0, t0, %lo(rust_main)
	jr t0

	.section .bss.stack
	.align 12
	.global bootstack
bootstack:
	.space 4096 * 32
	.global bootstacktop
bootstacktop:

