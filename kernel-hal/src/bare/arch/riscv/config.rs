/// Configuration of HAL.
#[derive(Debug)]
pub struct KernelConfig {
    pub phys_mem_start: usize,
    pub phys_mem_end: usize,
    pub phys_to_virt_offset: usize,
    pub dtb_paddr: usize,
}
