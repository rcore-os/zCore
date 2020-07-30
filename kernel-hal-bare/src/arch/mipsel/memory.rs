#[allow(dead_code)]
extern "C" {
    fn _root_page_table_buffer();
    fn _root_page_table_ptr();
}

pub unsafe fn set_page_table(vmtoken: usize) {
    use mips::tlb::TLBEntry;
    TLBEntry::clear_all();
    *(_root_page_table_ptr as *mut usize) = vmtoken;
}

pub fn get_page_table() -> usize {
    unsafe { *(_root_page_table_ptr as *mut usize) }
}
