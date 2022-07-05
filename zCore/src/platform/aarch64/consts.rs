// aarch64

pub const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 32 MB

#[inline]
pub fn phys_memory_base() -> usize {
    kernel_hal::arch::config::PHYS_MEMORY_BASE
}
