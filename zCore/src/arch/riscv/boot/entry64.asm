	.section .text.entry
	.globl _start
_start:
	#关闭mmu
        #csrw satp, zero

	#la sp, bootstacktop
	#call rust_main

	#可清零低12位地址
	lui t0, %hi(boot_page_table_sv39)
	li t1, 0xffffffff00000000 #立即数加载
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
	.space 1024 * 16
	.global bootstacktop
bootstacktop:


	.section .data
	.align 12 #12位对齐
boot_page_table_sv39:
	# 0xffffffff_00000000 --> 0x00000000
	# 0xffffffff_40000000 --> 0x40000000
	# 0xffffffff_80000000 --> 0x80000000
	# 0xffffffff_C0000000 --> 0xC0000000
	.zero 8 * 508
	.quad (0x00000 << 10) | 0xef
	.quad (0x40000 << 10) | 0xef
	.quad (0x80000 << 10) | 0xef
	.quad (0xc0000 << 10) | 0xef
