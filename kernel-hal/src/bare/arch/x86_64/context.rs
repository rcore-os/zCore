use x86_64::registers::control::Cr2;

use crate::VirtAddr;

hal_fn_impl! {
    impl mod crate::hal_fn::context {
        fn fetch_fault_vaddr() -> VirtAddr {
            Cr2::read().as_u64() as _
        }
    }
}
