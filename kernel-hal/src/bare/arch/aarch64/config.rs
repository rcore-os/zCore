//! Kernel configuration.

/// Kernel configuration passed by kernel when calls [`crate::primary_init_early()`].
#[derive(Debug)]
pub struct KernelConfig {
    pub rt_services_addr: usize,
    pub rsdp_addr: usize,
    pub phys_to_virt_offset: usize,
}

pub const UART_ADDR: usize = 0xffff_0000_0900_0000;