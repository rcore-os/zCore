ENTRY(_start)

IMAGE_BEGIN = 0x400000;

SECTIONS {

  . = IMAGE_BEGIN;

  .rodata ALIGN(4K):
  {
    *(.rodata .rodata.*)
  }

  .text ALIGN(4K):
  {
    stext = .;
    *(.text .text.*)
    etext = .;
  }

  .data ALIGN(4K):
  {
    PROVIDE_HIDDEN(kcounter_descriptor_begin = .);
    KEEP(*(.kcounter.descriptor))
    PROVIDE_HIDDEN(kcounter_descriptor_end = .);
    
    *(.data .data.*)
  }

  .got ALIGN(4K):
  {
    *(.got .got.*)
  }

  .bss ALIGN(4K):
  {
    *(.bss .bss.*)
  }

  .tbss :
  {
    *(.tbss .tbss.*)
  }
}
