use x86_64::registers::control::Cr2;

use crate::VirtAddr;

hal_fn_impl! {
    impl mod crate::defs::context {
        fn context_run(context: &mut UserContext) {
            context.run();
        }

        fn fetch_fault_vaddr() -> VirtAddr {
            Cr2::read().as_u64() as _
        }
    }
}
