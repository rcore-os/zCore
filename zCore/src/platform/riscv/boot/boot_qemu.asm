.equ PHY_MEM_OFS, 0xffffffff00000000

	.section .data
	.align 12
boot_page_table_sv39:
	.zero 8
	.zero 8
	.quad (0x80000 << 10) | 0xef #0x0000_0000_8000_0000 --> 0x8000_0000
	.zero 8 * 507
	.quad (0x80000 << 10) | 0xef #0xffff_ffff_8000_0000 --> 0x8000_0000
	.zero 8
