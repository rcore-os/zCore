/* Simple linker script for the ucore kernel.
   See the GNU ld 'info' manual ("info ld") to learn the syntax. */

OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE_ADDRESS = 0x80100000;

SECTIONS
{

    . = BASE_ADDRESS;
    start = .;

    .text : {
        stext = .;
        *(.text.entry)
        . = ALIGN(4K);
        *(.text.ebase)
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
        *(.dtb)
        . = ALIGN(4K);
        erodata = .;
    }

    .data ALIGN(4K): {
        sdata = .;
        PROVIDE_HIDDEN(kcounter_descriptor_begin = .);
        KEEP(*(.kcounter.descriptor))
        PROVIDE_HIDDEN(kcounter_descriptor_end = .);
        *(.data .data.*)
        edata = .;
    }

    .stack : {
        *(.bss.stack)
    }

    .bss : {
        sbss = .;
        *(.bss .bss.*)
        ebss = .;
    }

    PROVIDE(end = .);
}
