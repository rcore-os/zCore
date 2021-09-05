use riscv::register::{scause, stval};

use crate::VirtAddr;

hal_fn_impl! {
    impl mod crate::defs::context {
        fn context_run(context: &mut UserContext) {
            context.run();
        }

        fn fetch_fault_vaddr() -> VirtAddr {
            stval::read() as _
        }

        fn fetch_trap_num(_context: &UserContext) -> usize {
            scause::read().bits()
        }
    }
}
