/// MIPS is special
#[inline]
pub fn phys_to_virt(paddr: usize) -> usize {
    const PHYSICAL_MEMORY_OFFSET: usize = 0x8000_0000;
    if paddr <= PHYSICAL_MEMORY_OFFSET {
        PHYSICAL_MEMORY_OFFSET + paddr
    } else {
        paddr
    }
}
