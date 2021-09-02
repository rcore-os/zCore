pub use crate::common::context::*;

pub use trapframe::syscall_fn_entry as syscall_entry;

pub fn context_run(context: &mut UserContext) {
    context.run_fncall();
}

/// Get fault address of the last page fault.
pub fn fetch_fault_vaddr() -> crate::VirtAddr {
    unimplemented!()
}

/// Get the trap number when trap.
pub fn fetch_trap_num(_context: &UserContext) -> usize {
    unimplemented!()
}
