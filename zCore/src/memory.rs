//! Define physical frame allocation and dynamic memory allocation.

use core::ops::Range;

use bitmap_allocator::BitAlloc;
use kernel_hal::PhysAddr;
use spin::Mutex;

use super::platform::consts::*;

#[cfg(target_arch = "x86_64")]
type FrameAlloc = bitmap_allocator::BitAlloc16M; // max 64G

#[cfg(target_arch = "riscv64")]
type FrameAlloc = bitmap_allocator::BitAlloc1M; // max 4G

const PAGE_SIZE: usize = 4096;

/// Global physical frame allocator
static FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::DEFAULT);

fn phys_addr_to_frame_idx(addr: PhysAddr) -> usize {
    (addr - PHYS_MEMORY_BASE) / PAGE_SIZE
}

fn frame_idx_to_phys_addr(idx: usize) -> PhysAddr {
    idx * PAGE_SIZE + PHYS_MEMORY_BASE
}

pub fn init_frame_allocator(regions: &[Range<PhysAddr>]) {
    let mut ba = FRAME_ALLOCATOR.lock();
    for region in regions {
        let frame_start = phys_addr_to_frame_idx(region.start);
        let frame_end = phys_addr_to_frame_idx(region.end - 1) + 1;
        assert!(frame_start < frame_end, "illegal range for frame allocator");
        ba.insert(frame_start..frame_end);
        info!(
            "Frame allocator: add range {:#x?}",
            frame_idx_to_phys_addr(frame_start)..frame_idx_to_phys_addr(frame_end),
        );
    }
    info!("Frame allocator init end.");
}

pub fn frame_alloc() -> Option<PhysAddr> {
    let ret = FRAME_ALLOCATOR.lock().alloc().map(frame_idx_to_phys_addr);
    trace!("frame_alloc(): {:x?}", ret);
    ret
}

pub fn frame_alloc_contiguous(frame_count: usize, align_log2: usize) -> Option<PhysAddr> {
    let ret = FRAME_ALLOCATOR
        .lock()
        .alloc_contiguous(frame_count, align_log2)
        .map(frame_idx_to_phys_addr);
    trace!(
        "frame_alloc_contiguous(): {:x?} ~ {:x?}, align_log2={}",
        ret,
        ret.map(|x| x + frame_count),
        align_log2,
    );
    ret
}

pub fn frame_dealloc(target: PhysAddr) {
    trace!("frame_dealloc(): {:x}", target);
    FRAME_ALLOCATOR
        .lock()
        .dealloc(phys_addr_to_frame_idx(target))
}

cfg_if! {
    if #[cfg(not(feature = "libos"))] {
        use buddy_system_allocator::LockedHeap;

        /// Global heap allocator
        ///
        /// Available after `memory::init_heap()`.
        #[global_allocator]
        static HEAP_ALLOCATOR: LockedHeap = LockedHeap::new();

        /// Initialize the global heap allocator.
        pub fn init_heap() {
            const MACHINE_ALIGN: usize = core::mem::size_of::<usize>();
            const HEAP_BLOCK: usize = KERNEL_HEAP_SIZE / MACHINE_ALIGN;
            static mut HEAP: [usize; HEAP_BLOCK] = [0; HEAP_BLOCK];
            let heap_start = unsafe { HEAP.as_ptr() as usize };
            unsafe {
                HEAP_ALLOCATOR
                    .lock()
                    .init(heap_start, HEAP_BLOCK * MACHINE_ALIGN);
            }
            info!(
                "Heap init end: {:#x?}",
                heap_start..heap_start + KERNEL_HEAP_SIZE
            );
        }
    } else {
        pub fn init_heap() {}
    }
}

#[cfg(feature = "hypervisor")]
mod rvm_extern_fn {
    use super::*;

    #[rvm::extern_fn(alloc_frame)]
    fn rvm_alloc_frame() -> Option<usize> {
        hal_frame_alloc()
    }

    #[rvm::extern_fn(dealloc_frame)]
    fn rvm_dealloc_frame(paddr: usize) {
        hal_frame_dealloc(&paddr)
    }

    #[rvm::extern_fn(phys_to_virt)]
    fn rvm_phys_to_virt(paddr: usize) -> usize {
        paddr + PHYSICAL_MEMORY_OFFSET
    }

    #[cfg(target_arch = "x86_64")]
    #[rvm::extern_fn(is_host_timer_interrupt)]
    fn rvm_is_host_timer_interrupt(vector: u8) -> bool {
        vector == 32 // IRQ0 + Timer in kernel-hal-bare/src/arch/x86_64/interrupt.rs
    }

    #[cfg(target_arch = "x86_64")]
    #[rvm::extern_fn(is_host_serial_interrupt)]
    fn rvm_is_host_serial_interrupt(vector: u8) -> bool {
        vector == 36 // IRQ0 + COM1 in kernel-hal-bare/src/arch/x86_64/interrupt.rs
    }
}
