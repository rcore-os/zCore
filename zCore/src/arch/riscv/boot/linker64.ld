OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE_ADDRESS = 0x80010000;

/* Qemu
BASE_ADDRESS = 0x80200000;
*/

/* opensbi仍旧把kernel放在0x80200000物理内存中 */
BASE_ADDRESS = 0xffffffff80200000;

SECTIONS
{
    /* Load the kernel at this address: "." means the current address */
    . = BASE_ADDRESS;
    start = .;

    .text : {
        stext = .;
        *(.text.entry)
    _copy_user_start = .;
    *(.text.copy_user)
    _copy_user_end = .;
        *(.text .text.*)
        . = ALIGN(4K);
        etext = .;
    }

    .rodata : {
        srodata = .;
        *(.rodata .rodata.*)
        erodata = .;
    }

    .data : {
        sdata = .;
        *(.data .data.*)
        edata = .;
    }

    .stack : {
        *(.bss.stack)
        . = ALIGN(4K);
    }

    .bss : {
        sbss = .;
        *(.bss .bss.*)
        . = ALIGN(4K);
        ebss = .;
    }

    PROVIDE(end = .);
}
