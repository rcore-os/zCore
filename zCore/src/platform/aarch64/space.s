.section .data
.align 12
sdata:
    .space 0x8000 // 32K

.section .bss.stack
.align 12
boot_stack:
    .space 0x8000 // 32K
boot_stack_top: