//! Kernel configuration.

/// Kernel configuration passed by kernel when calls [`crate::primary_init_early()`].
#[derive(Debug)]
pub struct KernelConfig {
    pub phys_to_virt_offset: usize,
    pub dtb_paddr: usize,
}
