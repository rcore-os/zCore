//! Zircon HAL implementation for bare metal environment.
//!
//! This crate implements the following interfaces:
//! - `hal_pt_new`
//! - `hal_pt_map`
//! - `hal_pt_unmap`
//! - `hal_pt_protect`
//! - `hal_pt_query`
//! - `hal_pmem_read`
//! - `hal_pmem_write`
//!
//! And you have to implement these interfaces in addition:
//! - `hal_pt_map_kernel`
//! - `hal_pmem_base`

#![no_std]
#![feature(asm)]
#![feature(linkage)]
#![deny(warnings)]

extern crate log;

extern crate alloc;

#[cfg(target_arch = "x86_64")]
#[path = "arch/x86_64.rs"]
mod arch;

type PhysAddr = usize;
type VirtAddr = usize;

/// Map kernel for the new page table.
///
/// `pt` is a page-aligned pointer to the root page table.
#[linkage = "weak"]
#[export_name = "hal_pt_map_kernel"]
pub fn map_kernel(_pt: *mut u8) {
    unimplemented!()
}

#[repr(C)]
pub struct Frame {
    paddr: PhysAddr,
}

impl Frame {
    #[linkage = "weak"]
    #[export_name = "hal_frame_alloc"]
    pub fn alloc() -> Option<Self> {
        unimplemented!()
    }

    #[linkage = "weak"]
    #[export_name = "hal_frame_dealloc"]
    pub fn dealloc(&mut self) {
        unimplemented!()
    }
}

/// Map physical memory from here.
#[linkage = "weak"]
#[export_name = "hal_pmem_base"]
pub static PMEM_BASE: VirtAddr = 0x8_00000000;

fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    PMEM_BASE + paddr
}

/// Read physical memory from `paddr` to `buf`.
#[export_name = "hal_pmem_read"]
pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
    unsafe {
        (phys_to_virt(paddr) as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
    }
}

/// Write physical memory to `paddr` from `buf`.
#[export_name = "hal_pmem_write"]
pub fn pmem_write(paddr: PhysAddr, buf: &[u8]) {
    unsafe {
        buf.as_ptr()
            .copy_to_nonoverlapping(phys_to_virt(paddr) as _, buf.len());
    }
}
