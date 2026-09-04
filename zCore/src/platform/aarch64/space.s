.section .data
.align 16
sdata:
    .space 0x8000 // 32K

.section .bss.stack
.align 16
boot_stack:
    .space 0x8000 // 32K
boot_stack_top: