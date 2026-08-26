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

    pub qid: usize,

    pub cq_head: usize,

    pub cq_phase: usize,

    pub sq_tail: usize,

    /// Per-queue command identifier counter (NVMe requires unique CIDs among
    /// outstanding commands; we run one command at a time per queue).
    pub cid_counter: u16,

    pub sq_pa: usize,

    pub cq_pa: usize,

    /// DMA bounce buffer backing this queue's data transfers.
    pub data_pa: usize,
    pub data_va: usize,
    pub data_len: usize,

    /// One DMA page used as the command's PRP list when a transfer spans more
    /// than two pages (PRP1 = page 0, PRP2 -> this list with the remaining
    /// page addresses). 512 u64 entries fit; the 32-page bounce needs 31.
    pub prp_list_pa: usize,
    pub prp_list_va: usize,
}

impl<P: Provider> NvmeQueue<P> {
    pub fn new(qid: usize, q_size: usize) -> Self {
        // SQ: 64 bytes per entry. CQ: 16 bytes per entry.
        let sq_bytes = q_size * 64;
        let cq_bytes = q_size * 16;

        // Round up to page size
        let sq_pages = sq_bytes.div_ceil(P::PAGE_SIZE);
        let cq_pages = cq_bytes.div_ceil(P::PAGE_SIZE);

        // 32 pages (128 KiB), not 2: with a 2-page bounce every 1 MiB
        // read-ahead window became 128 serialized 8 KiB commands — enough
        // per-command latency to leave NVMe SLOWER than SATA. 128 KiB per
        // command needs a PRP list (below); `io_rw` still clamps each command
        // to the controller's advertised MDTS.
        let data_len = P::PAGE_SIZE * 32;
        let (data_va, data_pa) = P::alloc_dma(data_len);
        let (prp_list_va, prp_list_pa) = P::alloc_dma(P::PAGE_SIZE);
        let (sq_va, sq_pa) = P::alloc_dma(sq_pages * P::PAGE_SIZE);
        let (cq_va, cq_pa) = P::alloc_dma(cq_pages * P::PAGE_SIZE);

        trace!(
            "data_va: {:x}, sq_pa: {:x}, cq_pa: {:x}",
            data_va,
            sq_pa,
            cq_pa
        );

        // Completion queue memory must start zeroed so the phase-bit polling
        // doesn't mistake stale data for a valid completion.
        unsafe {
            core::ptr::write_bytes(sq_va as *mut u8, 0, sq_pages * P::PAGE_SIZE);
            core::ptr::write_bytes(cq_va as *mut u8, 0, cq_pages * P::PAGE_SIZE);
        }

        let submit_queue =
            unsafe { slice::from_raw_parts_mut(sq_va as *mut Volatile<NvmeCommonCommand>, q_size) };

        let complete_queue =
            unsafe { slice::from_raw_parts_mut(cq_va as *mut Volatile<NvmeCompletion>, q_size) };

        NvmeQueue {
            provider: PhantomData,
            sq: submit_queue,
            cq: complete_queue,
            qid,
            cq_head: 0,
            cq_phase: 1, // Phase starts at 1
            sq_tail: 0,
            cid_counter: 0,
            sq_pa,
            cq_pa,
            data_pa,
            data_va,
            data_len,
            prp_list_pa,
            prp_list_va,
        }
    }

    pub fn next_cid(&mut self) -> u16 {
        self.cid_counter = self.cid_counter.wrapping_add(1);
        self.cid_counter
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
