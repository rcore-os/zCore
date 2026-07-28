//! DMA memory region allocator.
//!
//! Provides page-aligned DMA-capable buffers backed by the kernel DMA allocator
//! (`drivers_dma_alloc`).  Physical and virtual addresses are tracked so that
//! hardware registers can be programmed with the physical address while the CPU
//! accesses data through the virtual address.

use crate::bus::PAGE_SIZE;

extern "C" {
    fn drivers_dma_alloc(pages: usize) -> usize;
    fn drivers_dma_dealloc(paddr: usize, pages: usize) -> i32;
    fn drivers_phys_to_virt(paddr: usize) -> usize;
    fn drivers_dma_mark_uncached(paddr: usize, pages: usize) -> i32;
    fn drivers_dma_verify_uncached(paddr: usize, pages: usize) -> i32;
}

/// A contiguous, page-aligned DMA memory region.
pub struct DmaRegion {
    virt: usize,
    phys: usize,
    pages: usize,
}

impl DmaRegion {
    /// Allocate `len` bytes of DMA-capable memory, zero-filled.
    pub fn alloc(len: usize) -> Option<Self> {
        Self::alloc_inner(len, true)
    }

    /// Allocate without zeroing. Use for RX buffers filled by device DMA: zeroing
    /// dirties the cache and breaks coherency on x86 unless mappings are UC.
    pub fn alloc_uninit(len: usize) -> Option<Self> {
        Self::alloc_inner(len, false)
    }

    fn alloc_inner(len: usize, zero: bool) -> Option<Self> {
        if len == 0 {
            return None;
        }
        let pages = len.div_ceil(PAGE_SIZE);
        let phys = unsafe { drivers_dma_alloc(pages) };
        if phys == 0 {
            return None;
        }
        if phys & (PAGE_SIZE - 1) != 0 {
            unsafe { drivers_dma_dealloc(phys, pages) };
            return None;
        }
        let virt = unsafe { drivers_phys_to_virt(phys) };
        if zero {
            unsafe { core::ptr::write_bytes(virt as *mut u8, 0, pages * PAGE_SIZE) };
        }
        // Evict any stale — possibly *dirty* — cache lines this physical memory
        // carried from its previous life (it is recycled from the frame
        // allocator and `alloc_uninit` does not zero it) BEFORE any device DMAs
        // into it. Otherwise a later `dma_sync(FromDevice)` clflush would write
        // such a dirty line back to RAM *over* the bytes the device just DMA'd
        // in — silent RX corruption that scales with the number of buffers
        // touched, so a large transfer (many buffers) fails while a small one
        // (few buffers) slips through. Writing the zeros/garbage back to RAM now
        // is harmless: the device overwrites it on the next receive. This also
        // covers the WB->UC transition in `map_coherent`, which does not flush
        // the cache itself. On non-x86 this is just a fence (those rely on UC
        // mappings); see `dma_sync`.
        crate::utils::dma_sync::dma_sync_wb_to_device(virt, pages * PAGE_SIZE);
        Some(Self { virt, phys, pages })
    }

    /// Virtual (CPU-accessible) base address of the region.
    #[inline]
    pub fn vaddr(&self) -> usize {
        self.virt
    }

    /// Physical (device-accessible) base address of the region.
    #[inline]
    pub fn paddr(&self) -> usize {
        self.phys
    }

    /// Size of the allocation in bytes (always page-rounded up).
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.pages * PAGE_SIZE
    }

    /// Return a raw pointer to the start of the region cast to `*mut T`.
    #[inline]
    pub fn as_ptr<T>(&self) -> *mut T {
        self.virt as *mut T
    }

    /// Map this region uncacheable in the kernel page tables (bare-metal NIC DMA).
    pub fn mark_uncached(&self) -> bool {
        if self.phys == 0 {
            return false;
        }
        unsafe { drivers_dma_mark_uncached(self.phys, self.pages) == 0 }
    }

    /// Linux `dma_alloc_coherent` / FreeBSD `BUS_DMA_COHERENT`: map UC at alloc time.
    pub fn map_coherent(&self) -> bool {
        self.mark_uncached() && self.verify_uncached()
    }

    /// Allocate and map UC immediately (preferred for NIC rings — no WB fallback).
    pub fn alloc_coherent(len: usize) -> Option<Self> {
        let region = Self::alloc(len)?;
        if region.map_coherent() {
            Some(region)
        } else {
            None
        }
    }

    /// Like [`Self::alloc_uninit`] but returns `(region, coherent)` even if PAT remap fails.
    pub fn alloc_uninit_try_coherent(len: usize) -> Option<(Self, bool)> {
        let region = Self::alloc_uninit(len)?;
        let coherent = region.map_coherent();
        Some((region, coherent))
    }

    /// Returns true when every page in the region is mapped UC/UC- in the PTEs.
    pub fn verify_uncached(&self) -> bool {
        if self.phys == 0 {
            return false;
        }
        unsafe { drivers_dma_verify_uncached(self.phys, self.pages) == 0 }
    }
}

impl Drop for DmaRegion {
    fn drop(&mut self) {
        if self.phys != 0 {
            unsafe { drivers_dma_dealloc(self.phys, self.pages) };
        }
    }
}
