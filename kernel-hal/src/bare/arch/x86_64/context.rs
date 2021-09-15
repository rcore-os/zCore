use bitflags::bitflags;
use x86_64::registers::control::Cr2;

use crate::{MMUFlags, VirtAddr};

bitflags! {
    struct PageFaultErrorCode: u32 {
        const PRESENT =     1 << 0;
        const WRITE =       1 << 1;
        const USER =        1 << 2;
        const RESERVED =    1 << 3;
        const INST =        1 << 4;
    }
}

hal_fn_impl! {
    impl mod crate::hal_fn::context {
        fn fetch_page_fault_info(error_code: usize) -> (VirtAddr, MMUFlags) {
            let fault_vaddr = Cr2::read().as_u64() as _;
            let mut flags = MMUFlags::empty();
            let code = PageFaultErrorCode::from_bits_truncate(error_code as u32);
            if code.contains(PageFaultErrorCode::WRITE) {
                flags |= MMUFlags::WRITE
            } else {
                flags |= MMUFlags::READ
            }
            if code.contains(PageFaultErrorCode::USER) {
                flags |= MMUFlags::USER
            }
            if code.contains(PageFaultErrorCode::INST) {
                flags |= MMUFlags::EXECUTE
            }
            if code.contains(PageFaultErrorCode::RESERVED) {
                error!("page table entry has reserved bits set!")
            }
            (fault_vaddr, flags)
        }
    }
}
