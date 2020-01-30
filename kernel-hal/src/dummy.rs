use super::*;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ops::FnOnce;
use core::time::Duration;

type ThreadId = usize;

#[repr(C)]
pub struct Thread {
    id: ThreadId,
}

impl Thread {
    /// Spawn a new thread.
    #[linkage = "weak"]
    #[export_name = "hal_thread_spawn"]
    pub fn spawn<T>(
        _self: Arc<T>,
        _entry: usize,
        _stack: usize,
        _arg1: usize,
        _arg2: usize,
        _tp: usize,
    ) -> Self {
        unimplemented!()
    }

    /// Exit current thread.
    #[linkage = "weak"]
    #[export_name = "hal_thread_exit"]
    pub fn exit() -> ! {
        unimplemented!()
    }

    /// Get TLS variable of current thread passed from `spawn`.
    #[linkage = "weak"]
    #[export_name = "hal_thread_tls"]
    pub fn tls<T>() -> Arc<T> {
        unimplemented!()
    }

    /// Blocks current thread.
    #[linkage = "weak"]
    #[export_name = "hal_thread_park"]
    pub fn park() {
        unimplemented!()
    }

    /// Get a waker of current thread.
    #[linkage = "weak"]
    #[export_name = "hal_thread_get_waker"]
    pub fn get_waker() -> Waker {
        unimplemented!()
    }
}

/// A handle for waking up thread.
#[repr(C)]
pub struct Waker {
    id: ThreadId,
}

impl Waker {
    /// Wake up related thread.
    #[linkage = "weak"]
    #[export_name = "hal_thread_wake"]
    pub fn wake(&self) {
        unimplemented!()
    }
}

/// Page Table
#[repr(C)]
pub struct PageTable {
    table_phys: PhysAddr,
}

impl PageTable {
    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[linkage = "weak"]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        unimplemented!()
    }
    /// Map the page of `vaddr` to the frame of `paddr` with `flags`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_map"]
    pub fn map(&mut self, _vaddr: VirtAddr, _paddr: PhysAddr, _flags: MMUFlags) -> Result<(), ()> {
        unimplemented!()
    }
    /// Unmap the page of `vaddr`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_unmap"]
    pub fn unmap(&mut self, _vaddr: VirtAddr) -> Result<(), ()> {
        unimplemented!()
    }
    /// Change the `flags` of the page of `vaddr`.
    #[linkage = "weak"]
    #[export_name = "hal_pt_protect"]
    pub fn protect(&mut self, _vaddr: VirtAddr, _flags: MMUFlags) -> Result<(), ()> {
        unimplemented!()
    }
    /// Query the physical address which the page of `vaddr` maps to.
    #[linkage = "weak"]
    #[export_name = "hal_pt_query"]
    pub fn query(&mut self, _vaddr: VirtAddr) -> Result<PhysAddr, ()> {
        unimplemented!()
    }
    /// Get the physical address of root page table.
    pub fn table_phys(&self) -> PhysAddr {
        self.table_phys
    }

    pub fn map_many(
        &mut self,
        mut vaddr: VirtAddr,
        paddrs: &[PhysAddr],
        flags: MMUFlags,
    ) -> Result<(), ()> {
        for &paddr in paddrs {
            self.map(vaddr, paddr, flags)?;
            vaddr += PAGE_SIZE;
        }
        Ok(())
    }

    pub fn map_cont(
        &mut self,
        mut vaddr: VirtAddr,
        paddr: PhysAddr,
        pages: usize,
        flags: MMUFlags,
    ) -> Result<(), ()> {
        for i in 0..pages {
            let paddr = paddr + i * PAGE_SIZE;
            self.map(vaddr, paddr, flags)?;
            vaddr += PAGE_SIZE;
        }
        Ok(())
    }

    pub fn unmap_cont(&mut self, vaddr: VirtAddr, pages: usize) -> Result<(), ()> {
        for i in 0..pages {
            self.unmap(vaddr + i * PAGE_SIZE)?;
        }
        Ok(())
    }
}

#[repr(C)]
pub struct PhysFrame {
    paddr: PhysAddr,
}

impl PhysFrame {
    #[linkage = "weak"]
    #[export_name = "hal_frame_alloc"]
    pub fn alloc() -> Option<Self> {
        unimplemented!()
    }

    pub fn addr(&self) -> PhysAddr {
        self.paddr
    }
}

impl Drop for PhysFrame {
    #[linkage = "weak"]
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        unimplemented!()
    }
}

/// Read physical memory from `paddr` to `buf`.
#[linkage = "weak"]
#[export_name = "hal_pmem_read"]
pub fn pmem_read(_paddr: PhysAddr, _buf: &mut [u8]) {
    unimplemented!()
}

/// Write physical memory to `paddr` from `buf`.
#[linkage = "weak"]
#[export_name = "hal_pmem_write"]
pub fn pmem_write(_paddr: PhysAddr, _buf: &[u8]) {
    unimplemented!()
}

/// Output a char to console.
#[linkage = "weak"]
#[export_name = "hal_serial_write"]
pub fn serial_write(_s: &str) {
    unimplemented!()
}

/// Get current time.
#[linkage = "weak"]
#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    unimplemented!()
}

/// Set a new timer. After `deadline`, the `callback` will be called.
#[linkage = "weak"]
#[export_name = "hal_timer_set"]
pub fn timer_set(_deadline: Duration, _callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
    unimplemented!()
}

/// Set FSBASE on user space.
#[linkage = "weak"]
#[export_name = "hal_set_user_fsbase"]
pub fn set_user_fsbase(_fsbase: usize) {
    unimplemented!()
}
