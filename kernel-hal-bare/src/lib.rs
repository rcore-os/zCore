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

#[macro_use]
extern crate log;

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::{future::Future, pin::Pin};
use kernel_hal::defs::*;

pub mod arch;

#[allow(improper_ctypes)]
extern "C" {
    fn hal_pt_map_kernel(pt: *mut u8, current: *const u8);
    fn hal_frame_alloc() -> Option<PhysAddr>;
    fn hal_frame_dealloc(paddr: &PhysAddr);
    #[link_name = "hal_pmem_base"]
    static PMEM_BASE: usize;
}

#[repr(C)]
pub struct Thread {
    thread: usize,
}

impl Thread {
    #[export_name = "hal_thread_spawn"]
    pub fn spawn(thread: Arc<usize>, regs: GeneralRegs, vmtoken: usize) -> Self {
        executor::spawn(async move {
            unsafe {
                // TODO: switch page table between processes
                arch::set_page_table(vmtoken);
            }
            let mut context = trapframe::UserContext {
                // safety: same structure
                general: unsafe { core::mem::transmute(regs) },
                ..Default::default()
            };
            loop {
                // 判断线程状态是否是RUNNABLE,不是则返回Pending
                unsafe {
                    thread_check_runnable(&thread).await;
                }
                context.run();
                let exit = unsafe { handle_syscall(&thread, &mut context.general).await };
                if exit {
                    break;
                }
            }
        });
        Thread { thread: 0 }
    }
}

#[linkage = "weak"]
#[no_mangle]
extern "C" fn handle_syscall(
    _thread: &Arc<usize>,
    _regs: &mut trapframe::GeneralRegs,
) -> Pin<Box<dyn Future<Output = bool> + Send>> {
    // exit by default
    Box::pin(async { true })
}

/// Check whether a thread is runnable
#[linkage = "weak"]
#[no_mangle]
extern "C" fn thread_check_runnable(
    _thread: &Arc<usize>,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async {})
}

/// Map kernel for the new page table.
///
/// `pt` is a page-aligned pointer to the root page table.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn map_kernel(pt: *mut u8, current: *const u8) {
    unsafe {
        hal_pt_map_kernel(pt, current);
    }
}

#[repr(C)]
pub struct Frame {
    paddr: PhysAddr,
}

impl Frame {
    pub fn alloc() -> Option<Self> {
        unsafe { hal_frame_alloc().map(|paddr| Frame { paddr }) }
    }

    pub fn dealloc(&mut self) {
        unsafe {
            hal_frame_dealloc(&self.paddr);
        }
    }
}

fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    unsafe { PMEM_BASE + paddr }
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

/// Initialize the HAL.
pub fn init() {
    unsafe {
        trapframe::init();
    }
    arch::init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[no_mangle]
    extern "C" fn hal_pt_map_kernel(_pt: *mut u8, _current: *const u8) {
        unimplemented!()
    }

    #[no_mangle]
    extern "C" fn hal_frame_alloc() -> Option<PhysAddr> {
        unimplemented!()
    }

    #[no_mangle]
    extern "C" fn hal_frame_dealloc(_paddr: &PhysAddr) {
        unimplemented!()
    }

    #[export_name = "hal_pmem_base"]
    static PMEM_BASE: usize = 0;

    #[export_name = "hal_lapic_addr"]
    static LAPIC_ADDR: usize = 0;
}
