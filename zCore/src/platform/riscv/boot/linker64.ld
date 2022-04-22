OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    /* Load the kernel at this address: "." means the current address */
    . = BASE_ADDRESS;
    start = .;

    stext = .;
    .text : {
        *(.text.entry)
        *(.text .text.*)
    }

    . = ALIGN(4K);
    etext = .;
    srodata = .;
    .rodata : {
        *(.rodata .rodata.*)
    }

    . = ALIGN(4K);
    erodata = .;
    sdata = .;
    .data : {
        *(.data .data.*)
    }

    . = ALIGN(4K);
    edata = .;
    .bss : {
        *(.bss.bootstack)

        . = ALIGN(4K);
        sbss = .;
        *(.bss .bss.* .sbss)
    }

    . = ALIGN(4K);
    ebss = .;

    PROVIDE(end = .);
}
