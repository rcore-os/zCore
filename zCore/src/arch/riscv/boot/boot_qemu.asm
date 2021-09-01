.equ PHY_MEM_OFS, 0xffffffff00000000

	.section .data
	.align 12 #12位对齐
boot_page_table_sv39:
	#1G的一个大页: 0x00000000_80000000 --> 0x80000000
	#1G的一个大页: 0xffffffff_80000000 --> 0x80000000

	#前510项置0
	.zero 8
	.zero 8
	.quad (0x80000 << 10) | 0xef #0x80000000 --> 0x80000000

	.zero 8 * 507
	#倒数第二项，PPN=0x80000(当转换为物理地址时还需左移12位), 标志位DAG_XWRV置1
	.quad (0x80000 << 10) | 0xef
	.zero 8

