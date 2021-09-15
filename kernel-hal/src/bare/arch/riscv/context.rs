use riscv::register::scause::{Exception, Trap};
use riscv::register::{scause, stval};

use crate::{MMUFlags, VirtAddr};

hal_fn_impl! {
    impl mod crate::hal_fn::context {
        fn fetch_trap_num(_context: &UserContext) -> usize {
            scause::read().bits()
        }

        fn fetch_page_fault_info(_scause: usize) -> (VirtAddr, MMUFlags) {
            let fault_vaddr = stval::read() as _;
            let flags = match scause::read().cause() {
                Trap::Exception(Exception::LoadPageFault) => MMUFlags::READ,
                Trap::Exception(Exception::StorePageFault) => MMUFlags::WRITE,
                Trap::Exception(Exception::InstructionPageFault) => MMUFlags::EXECUTE,
                _ => unreachable!(),
            };
            (fault_vaddr, flags)
        }
    }
}
