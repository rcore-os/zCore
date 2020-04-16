ENTRY(_start)

KERNEL_BEGIN = 0xffffff0000000000;

SECTIONS {

  . = KERNEL_BEGIN;

  .rodata ALIGN(4K):
  {
    *(.rodata .rodata.*)
  }

  .text ALIGN(4K):
  {
    stext = .;
    _copy_user_start = .;
    *(.text.copy_user)
    _copy_user_end = .;
    *(.text .text.*)
    etext = .;
  }

  .data ALIGN(4K):
  {
    *(.data .data.*)
  }

  .kcounter.descriptor : ALIGN(4096) {
      PROVIDE_HIDDEN(kcounter_descriptor_begin = .);
      KEEP(*(.kcounter.descriptor.header))
      PROVIDE_HIDDEN(kcounter_descriptor_end = .);
  }

  .got ALIGN(4K):
  {
    *(.got .got.*)
  }

  .bss ALIGN(4K):
  {
	. = ALIGN(4096);
	PROVIDE_HIDDEN(kcounters_arena_start = .);
	KEEP(*(SORT_BY_NAME(.bss.kcounter.*)))
	PROVIDE_HIDDEN(kcounters_arena_end = .);
	. = ALIGN(4096);
	PROVIDE_HIDDEN(kcounters_arena_page_end = .);

    *(.bss .bss.*)
  }
}
