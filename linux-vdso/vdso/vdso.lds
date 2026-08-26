/*
 * Linker script for Eclipse's Linux-compatible vDSO.
 *
 * ONE PT_LOAD, starting at file offset 0 and virtual address 0, covering the
 * ELF header and the program headers (`FILEHDR PHDRS` on the segment below).
 * That is not cosmetic — it is what makes musl's lookup work:
 *
 *   for (i=0; i<eh->e_phnum; i++, ...) {
 *       if (ph->p_type == PT_LOAD)
 *           base = (size_t)eh + ph->p_offset - ph->p_vaddr;
 *       ...
 *   }
 *
 * musl derives the load bias from the LAST PT_LOAD it sees and resolves every
 * symbol against it, and `eh` is whatever `AT_SYSINFO_EHDR` points at. With a
 * single segment at 0/0 the bias is exactly the mapping base and the ELF header
 * sits at that base, so both hold by construction rather than by luck about how
 * the linker happened to lay several segments out. It also lets the kernel map
 * the image as one contiguous run of pages, with file offset == virtual offset,
 * which is what `map_from_elf` does.
 *
 * The segment is RX. The data page inside it is written by the kernel through
 * the shared VM object, never by userspace, so it needs no write permission in
 * any process — a process that could write it could lie to itself about the
 * time, and to nothing else, but there is no reason to allow even that.
 */

SECTIONS
{
    . = SIZEOF_HEADERS;

    /*
     * The dynamic-linking metadata musl walks. `.hash` is kept because musl
     * accepts either it or `.gnu.hash`; emitting the classic table costs a few
     * dozen bytes and removes a dependency on the toolchain's default
     * `--hash-style`.
     */
    .hash           : { *(.hash) }
    .gnu.hash       : { *(.gnu.hash) }
    .dynsym         : { *(.dynsym) }
    .dynstr         : { *(.dynstr) }
    .gnu.version    : { *(.gnu.version) }
    .gnu.version_d  : { *(.gnu.version_d) }
    .gnu.version_r  : { *(.gnu.version_r) }
    .dynamic        : { *(.dynamic) } :text :dynamic

    .rodata         : { *(.rodata*) }                       :text
    .text           : { *(.text*) }
    .altinstructions : { *(.altinstructions) }

    /*
     * The kernel-published clock parameters. Given its own page-aligned home so
     * the kernel can locate and rewrite it (via the `_vdso_data` symbol) without
     * caring how the rest of the image was laid out.
     */
    . = ALIGN(4096);
    .vdso_data      : { *(.vdso_data) }
    . = ALIGN(4096);

    /*
     * Nothing here should produce these. Discarding rather than ignoring them
     * means a future edit that accidentally introduces a relocation against an
     * external symbol, or a .bss, fails loudly at link time instead of
     * producing an image that silently misbehaves once mapped.
     */
    /DISCARD/ : {
        *(.note*)
        *(.eh_frame*)
        *(.comment)
        *(.debug*)
    }
}

PHDRS
{
    text    PT_LOAD    FILEHDR PHDRS FLAGS(5);   /* PF_R | PF_X */
    dynamic PT_DYNAMIC FLAGS(4);                 /* PF_R        */
}
