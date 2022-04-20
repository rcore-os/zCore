.equ PHY_MEM_OFS, 0xffffffff80000000

	.section .data
	.align 12
boot_page_table_sv39:
	# TODO d1 c906 有扩展 63:59 位的页表项属性
	#.quad (1 << 62) | (1 << 61) | (1 << 60) | (0x40000 << 10) | 0xef
	.zero 8
	.quad (0x40000 << 10) | 0xef # 0x0000_0000_4000_0000 --> 0x4000_0000
	.zero 8 * 508
	.quad (0x00000 << 10) | 0xef # 0xffff_ffff_8000_0000 --> 0x0000_0000
	.quad (0x40000 << 10) | 0xef # 0xffff_ffff_c000_0000 --> 0x4000_0000
