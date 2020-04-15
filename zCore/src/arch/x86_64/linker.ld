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

  /*
   * The named sections starting with kcountdesc are sorted by name so that
   * tools can provide binary search lookup for counters::Descriptor::name[]
   * variables.  This is page-aligned and padded out to page size so it can be
   * exported as a VMO without exposing any other .rodata contents.
   */
  .kcounter.desc : ALIGN(4096) {
      PROVIDE_HIDDEN(k_counter_desc_vmo_begin = .);
	  KEEP(*(.kcounter.desc.header))
	  ASSERT(. - k_counter_desc_vmo_begin == 16,
			 "lib/counters/counters.cc and kernel.ld mismatch");
      QUAD(_kcounter_desc_end - _kcounter_desc_start);
      PROVIDE_HIDDEN(_kcounter_desc_start = .);
	  ASSERT(_kcounter_desc_start - k_counter_desc_vmo_begin == 24,
			 "lib/counters/counters.cc and kernel.ld mismatch");
      KEEP(*(SORT_BY_NAME(kcountdesc.*)))
      PROVIDE_HIDDEN(_kcounter_desc_end = .);
      . = ALIGN(4096);
      PROVIDE_HIDDEN(k_counter_desc_vmo_end = .);
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
