//! Define the FrameAllocator for physical memory
//! x86_64      --  64GB

use crate::arch::consts::*;
use {bitmap_allocator::BitAlloc, buddy_system_allocator::LockedHeap, spin::Mutex};

#[cfg(target_arch = "x86_64")]
use rboot::{BootInfo, MemoryType};

#[cfg(target_arch = "x86_64")]
type FrameAlloc = bitmap_allocator::BitAlloc16M;

#[cfg(target_arch = "riscv64")]
type FrameAlloc = bitmap_allocator::BitAlloc1M;

static FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::DEFAULT);

#[cfg(target_arch = "x86_64")]
pub fn init_frame_allocator(boot_info: &BootInfo) {
    let mut ba = FRAME_ALLOCATOR.lock();
    for region in boot_info.memory_map.iter() {
        if region.ty == MemoryType::CONVENTIONAL {
            let start_frame = region.phys_start as usize / PAGE_SIZE;
            let end_frame = start_frame + region.page_count as usize;
            ba.insert(start_frame..end_frame);
            info!(
                "Frame allocator add range: {:#x?}",
                region.phys_start..region.phys_start + region.page_count * PAGE_SIZE as u64,
            );
        }
    }
    info!("Frame allocator init end");
}

#[cfg(target_arch = "riscv64")]
pub fn init_frame_allocator() {
    use core::ops::Range;
    use kernel_hal::addr::{align_down, align_up};

    /// Transform memory area `[start, end)` to integer range for `FrameAllocator`
    fn to_range(start: usize, end: usize) -> Range<usize> {
        info!("Frame allocator add range: {:#x?}", start..end);
        let page_start = (start - MEMORY_OFFSET) / PAGE_SIZE;
        let page_end = (end - MEMORY_OFFSET) / PAGE_SIZE;
        assert!(page_start < page_end, "illegal range for frame allocator");
        page_start..page_end
    }

    extern "C" {
        fn end();
    }

    let mut ba = FRAME_ALLOCATOR.lock();
    let mem_pool_start = align_up(end as usize + PAGE_SIZE - KERNEL_OFFSET + MEMORY_OFFSET);
    let mem_pool_end = align_down(MEMORY_END);
    ba.insert(to_range(mem_pool_start, mem_pool_end));

    info!("Frame allocator: init end");
}

pub fn init_heap() {
    const MACHINE_ALIGN: usize = core::mem::size_of::<usize>();
    const HEAP_BLOCK: usize = KERNEL_HEAP_SIZE / MACHINE_ALIGN;
    static mut HEAP: [usize; HEAP_BLOCK] = [0; HEAP_BLOCK];
    unsafe {
        HEAP_ALLOCATOR
            .lock()
            .init(HEAP.as_ptr() as usize, HEAP_BLOCK * MACHINE_ALIGN);
    }
    info!("heap init end");
}

pub fn frame_alloc() -> Option<usize> {
    // get the real address of the alloc frame
    let ret = FRAME_ALLOCATOR
        .lock()
        .alloc()
        .map(|id| id * PAGE_SIZE + MEMORY_OFFSET);
    trace!("Allocate frame: {:x?}", ret);
    ret
}

pub fn frame_alloc_contiguous(frame_count: usize, align_log2: usize) -> Option<usize> {
    let ret = FRAME_ALLOCATOR
        .lock()
        .alloc_contiguous(frame_count, align_log2)
        .map(|id| id * PAGE_SIZE + MEMORY_OFFSET);
    trace!(
        "Allocate contiguous frames: {:x?} ~ {:x?}",
        ret,
        ret.map(|x| x + frame_count)
    );
    ret
}

pub fn frame_dealloc(target: usize) {
    trace!("Deallocate frame: {:x}", target);
    FRAME_ALLOCATOR
        .lock()
        .dealloc((target - MEMORY_OFFSET) / PAGE_SIZE);
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

/// Global heap allocator
///
/// Available after `memory::init_heap()`.
#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap = LockedHeap::new();
