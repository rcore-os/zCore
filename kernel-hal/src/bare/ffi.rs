#[allow(improper_ctypes)]
extern "C" {
    pub fn hal_pt_map_kernel(pt: *mut u8, current: *const u8);
    pub fn hal_frame_alloc() -> Option<usize>;
    pub fn hal_frame_alloc_contiguous(frame_count: usize, align_log2: usize) -> Option<usize>;
    pub fn hal_frame_dealloc(paddr: usize);
    #[link_name = "hal_phys_to_virt_offset"]
    pub static PHYS_TO_VIRT_OFFSET: usize;
}
