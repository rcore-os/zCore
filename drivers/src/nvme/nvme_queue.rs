use alloc::slice;
use core::marker::PhantomData;
use volatile::Volatile;

use super::NvmeCommonCommand;
use super::NvmeCompletion;

#[derive(Debug)]
pub struct NvmeQueue<P: Provider> {
    provider: PhantomData<P>,

    pub sq: &'static mut [Volatile<NvmeCommonCommand>],
    pub cq: &'static mut [Volatile<NvmeCompletion>],

    pub db_offset: usize,

    pub qid: usize,

    pub cq_head: usize,

    pub cq_phase: usize,

    pub sq_tail: usize,

    pub last_sq_tail: usize,

    pub sq_pa: usize,

    pub cq_pa: usize,

    pub data_pa: usize,
}

impl<P: Provider> NvmeQueue<P> {
    pub fn new(qid: usize, db_offset: usize) -> Self {
        let (data_va, data_pa) = P::alloc_dma(P::PAGE_SIZE * 2);
        let (sq_va, sq_pa) = P::alloc_dma(P::PAGE_SIZE * 2);
        let (cq_va, cq_pa) = P::alloc_dma(P::PAGE_SIZE * 2);

        trace!("data_va: {:x}, data_pa: {:x}", data_va, data_pa);

        let submit_queue = unsafe {
            slice::from_raw_parts_mut(sq_va as *mut Volatile<NvmeCommonCommand>, PAGE_SIZE)
        };

        let complete_queue =
            unsafe { slice::from_raw_parts_mut(cq_va as *mut Volatile<NvmeCompletion>, PAGE_SIZE) };

        NvmeQueue {
            provider: PhantomData,
            sq: submit_queue,
            cq: complete_queue,
            db_offset,
            qid,
            cq_head: 0,
            cq_phase: 0,
            sq_tail: 0,
            last_sq_tail: 0,
            sq_pa,
            cq_pa,
            data_pa,
        }
    }
}

/// External functions that drivers must use
pub trait Provider {
    /// Page size (usually 4K)
    const PAGE_SIZE: usize;

    /// Allocate consequent physical memory for DMA.
    /// Return (`virtual address`, `physical address`).
    /// The address is page aligned.
    fn alloc_dma(size: usize) -> (usize, usize);

    /// Deallocate DMA
    fn dealloc_dma(vaddr: usize, size: usize);
}

pub struct ProviderImpl;

impl Provider for ProviderImpl {
    const PAGE_SIZE: usize = PAGE_SIZE;

    fn alloc_dma(size: usize) -> (usize, usize) {
        let paddr = unsafe { drivers_dma_alloc(size / PAGE_SIZE) };
        let vaddr = phys_to_virt(paddr);
        (vaddr, paddr)
    }

    fn dealloc_dma(vaddr: usize, size: usize) {
        let paddr = virt_to_phys(vaddr);
        unsafe { drivers_dma_dealloc(paddr, size / PAGE_SIZE) };
    }
}

pub fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    unsafe { drivers_phys_to_virt(paddr) }
}

pub fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
    unsafe { drivers_virt_to_phys(vaddr) }
}

pub fn timer_now_as_micros() -> u64 {
    unsafe { drivers_timer_now_as_micros() }
}

extern "C" {
    fn drivers_dma_alloc(pages: usize) -> PhysAddr;
    fn drivers_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32;
    fn drivers_phys_to_virt(paddr: PhysAddr) -> VirtAddr;
    fn drivers_virt_to_phys(vaddr: VirtAddr) -> PhysAddr;
    fn drivers_timer_now_as_micros() -> u64;
}

pub const PAGE_SIZE: usize = 4096;

type VirtAddr = usize;
type PhysAddr = usize;
