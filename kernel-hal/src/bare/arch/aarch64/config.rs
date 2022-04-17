//! Kernel configuration.
use crate::PAGE_SIZE;

/// Kernel configuration passed by kernel when calls [`crate::primary_init_early()`].
#[derive(Debug)]
pub struct KernelConfig {
    pub rt_services_addr: usize,
    pub rsdp_addr: usize,
    pub phys_to_virt_offset: usize,
}

pub const UART_ADDR: usize = 0xffff_0000_0900_0000;
pub const GIC_BASE: usize = 0xffff_0000_0800_0000;
pub const PA_1TB_BITS: usize = 40;
pub const PHYS_ADDR_MAX: usize = (1 << PA_1TB_BITS) - 1;
pub const PHYS_ADDR_MASK: usize = PHYS_ADDR_MAX & !(PAGE_SIZE - 1);
