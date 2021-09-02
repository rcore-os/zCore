pub use crate::common::context::*;

pub fn context_run(_context: &mut UserContext) {
    unimplemented!()
}

/// Get fault address of the last page fault.
pub fn fetch_fault_vaddr() -> crate::VirtAddr {
    unimplemented!()
}

/// Get the trap number when trap.
pub fn fetch_trap_num(_context: &UserContext) -> usize {
    unimplemented!()
}
