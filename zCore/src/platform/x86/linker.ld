ENTRY(_start)

KERNEL_BEGIN = 0xffffff0000000000;

SECTIONS {

  . = KERNEL_BEGIN;

  .text ALIGN(4K):
  {
    stext = .;
    _copy_user_start = .;
    *(.text.copy_user)
    _copy_user_end = .;
    *(.text .text.*)
    etext = .;
  }

  .rodata ALIGN(4K):
  {
    *(.rodata .rodata.*)
    *(.srodata .srodata.*)
  }

  .kcounter.desc ALIGN(4K):
  {
    PROVIDE_HIDDEN(kcounters_desc_vmo_start = .);
    KEEP(*(.kcounter.desc.header))
    QUAD(kcounters_desc_end - kcounters_desc_start);
    ASSERT(. - kcounters_desc_vmo_start == 24, "wrong size of the kcounter descriptor VMO header");

    PROVIDE_HIDDEN(kcounters_desc_start = .);
    KEEP(*(SORT_BY_NAME(.kcounter.desc.*)))
    PROVIDE_HIDDEN(kcounters_desc_end = .);
  } :rodata

  .data ALIGN(4K):
  {
    *(.data .data.*)
    *(.sdata .sdata.*)
  }

  .got ALIGN(4K):
  {
    *(.got .got.*)
  }

  .bss ALIGN(4K):
  {
    PROVIDE_HIDDEN(kcounters_arena_start = .);
    KEEP(*(SORT_BY_NAME(.bss.kcounter.*)))
    PROVIDE_HIDDEN(kcounters_arena_end = .);
    ASSERT(kcounters_arena_end - kcounters_arena_start == (kcounters_desc_end - kcounters_desc_start) * 8 / 64,
          "kcounters_arena size mismatch");

    *(.bss .bss.*)
    *(.sbss .sbss.*)
  }
}
