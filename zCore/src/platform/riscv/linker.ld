OUTPUT_ARCH(riscv)
ENTRY(_start)
INCLUDE zCore/src/platform/riscv/kernel-vars.ld
SECTIONS
{
    . = BASE_ADDRESS;
    start = .;

    .text : {
        stext = .;
        *(.text.entry)
        *(.text .text.*)
        etext = .;
    }

    .rodata ALIGN(4K) : {
        srodata = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        erodata = .;
    }

    .data ALIGN(4K) : {
        sdata = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        edata = .;
    }

    .stack ALIGN(4K) : {
        sstack = .;
        *(.bss.stack)
    }

    .bss ALIGN(4K) : {
        bootstack = .;
        *(.bss.bootstack)
        bootstacktop = .;

        . = ALIGN(4K);
        sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        ebss = .;
    }

    PROVIDE(end = .);
}
