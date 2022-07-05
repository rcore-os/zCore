// x86_64

pub const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB

#[inline]
pub fn phys_memory_base() -> usize {
    0
}
