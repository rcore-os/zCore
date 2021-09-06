use crate::{mem::PhysFrame, PhysAddr};

pub use super::arch::vm::*;
pub use crate::common::vm::*;

/// Page Table
pub struct PageTable {
    root: PhysFrame,
}

impl PageTable {
    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let root = PhysFrame::new_zero().expect("failed to alloc frame");
        Self { root }
    }

    fn map_kernel(&mut self) {
        let old_root_vaddr = crate::mem::phys_to_virt(crate::vm::current_vmtoken());
        let new_root_vaddr = crate::mem::phys_to_virt(self.root.paddr());
        unsafe { super::ffi::hal_pt_map_kernel(new_root_vaddr as _, old_root_vaddr as _) };
    }

    /// Create a new `PageTable`. and map kernel address space to it.
    pub fn new_and_map_kernel() -> Self {
        let mut pt = Self::new();
        pt.map_kernel();
        pt
    }

    unsafe fn from_root(root_paddr: PhysAddr) -> Self {
        Self {
            root: PhysFrame::from_paddr(root_paddr),
        }
    }

    /// Create a new `PageTable` from current VM token. (e.g. CR3, SATP, ...)
    pub fn from_current() -> Self {
        unsafe { Self::from_root(crate::vm::current_vmtoken()) }
    }
}

impl PageTableTrait for PageTable {
    fn table_phys(&self) -> PhysAddr {
        self.root.paddr()
    }
}
