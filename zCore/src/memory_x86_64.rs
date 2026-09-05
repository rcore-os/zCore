//! Define physical frame allocation and dynamic memory allocation.

use bitmap_allocator::BitAlloc;
use core::ops::Range;
#[cfg(not(feature = "libos"))]
use kernel_hal::mem::phys_to_virt;
use kernel_hal::sync::Mutex;
use kernel_hal::PhysAddr;

type FrameAlloc = bitmap_allocator::BitAlloc16M; // 16M frames

// LibOS uses the host page size.  In particular Apple Silicon has 16 KiB
// pages, so allocating frames in hard-coded 4 KiB units produces file offsets
// that mmap(2) rejects with EINVAL.
const PAGE_BITS: usize = kernel_hal::PAGE_SIZE.trailing_zeros() as usize;

/// Global physical frame allocator
static FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::DEFAULT);

#[inline]
fn phys_addr_to_frame_idx(addr: PhysAddr) -> usize {
    addr >> PAGE_BITS
}

#[inline]
fn frame_idx_to_phys_addr(idx: usize) -> PhysAddr {
    idx << PAGE_BITS
}

pub fn insert_regions(regions: &[Range<PhysAddr>]) {
    debug!("init_frame_allocator regions: {regions:x?}");
    let mut ba = FRAME_ALLOCATOR.lock();
    #[cfg(not(feature = "libos"))]
    const DYNAMIC_HEAP_SIZE: usize = 512 * 1024 * 1024;
    #[cfg(not(feature = "libos"))]
    const DYNAMIC_HEAP_ALIGN: usize = 256 * 1024 * 1024;
    #[cfg(not(feature = "libos"))]
    let heap_region = regions.iter().find_map(|region| {
        let start = (region.start + DYNAMIC_HEAP_ALIGN - 1) & !(DYNAMIC_HEAP_ALIGN - 1);
        (start + DYNAMIC_HEAP_SIZE <= region.end).then_some(start..start + DYNAMIC_HEAP_SIZE)
    });
    #[cfg(not(feature = "libos"))]
    if let Some(region) = &heap_region {
        unsafe {
            HEAP_ALLOCATOR
                .lock()
                .add_to_heap(phys_to_virt(region.start), phys_to_virt(region.end));
        }
        info!("Dynamic heap region: {region:#x?}");
    }
    #[cfg(feature = "libos")]
    let heap_region: Option<Range<PhysAddr>> = None;

    for region in regions {
        let frame_start = phys_addr_to_frame_idx(region.start);
        let frame_end = phys_addr_to_frame_idx(region.end - 1) + 1;

        if let Some(heap) = heap_region
            .as_ref()
            .filter(|heap| region.start <= heap.start && heap.end <= region.end)
        {
            let heap_start = phys_addr_to_frame_idx(heap.start);
            let heap_end = phys_addr_to_frame_idx(heap.end);
            if frame_start < heap_start {
                ba.insert(frame_start..heap_start);
            }
            if heap_end < frame_end {
                ba.insert(heap_end..frame_end);
            }
        } else if frame_start < frame_end {
            ba.insert(frame_start..frame_end);
        }
        if frame_start < frame_end {
            info!(
                "Frame allocator: add range {:#x?}",
                frame_idx_to_phys_addr(frame_start)..frame_idx_to_phys_addr(frame_end),
            );
        }
    }
    info!("Frame allocator init end.");
}

pub fn frame_alloc(frame_count: usize, align_log2: usize) -> Option<PhysAddr> {
    let ret = FRAME_ALLOCATOR
        .lock()
        .alloc_contiguous(frame_count, align_log2)
        .map(frame_idx_to_phys_addr);
    trace!(
        "frame_alloc_contiguous(): {ret:x?} ~ {end_ret:x?}, align_log2={align_log2}",
        end_ret = ret.map(|x| x + frame_count),
    );
    ret
}

pub fn frame_dealloc(target: PhysAddr) {
    trace!("frame_dealloc(): {target:x}");
    FRAME_ALLOCATOR
        .lock()
        .dealloc(phys_addr_to_frame_idx(target))
}

cfg_if! {
    if #[cfg(not(feature = "libos"))] {
        use buddy_system_allocator::Heap;
        use core::{
            alloc::{GlobalAlloc, Layout},
            ops::Deref,
            ptr::NonNull,
        };

        const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB
        const ORDER: usize = 32;

        /// Global heap allocator
        ///
        /// Available after `memory::init()`.
        #[global_allocator]
        static HEAP_ALLOCATOR: LockedHeap<ORDER> = LockedHeap::<ORDER>::new();

        /// Initialize the global heap allocator.
        pub fn init() {
            const MACHINE_ALIGN: usize = core::mem::size_of::<usize>();
            const HEAP_BLOCK: usize = KERNEL_HEAP_SIZE / MACHINE_ALIGN;
            static mut HEAP: [usize; HEAP_BLOCK] = [0; HEAP_BLOCK];
            let heap_start = (&raw const HEAP).cast::<u8>() as usize;
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

        pub struct LockedHeap<const ORDER: usize>(Mutex<Heap<ORDER>>);

        impl<const ORDER: usize> LockedHeap<ORDER> {
            /// Creates an empty heap
            pub const fn new() -> Self {
                LockedHeap(Mutex::new(Heap::<ORDER>::new()))
            }
        }

        impl<const ORDER: usize> Deref for LockedHeap<ORDER> {
            type Target = Mutex<Heap<ORDER>>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        unsafe impl<const ORDER: usize> GlobalAlloc for LockedHeap<ORDER> {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                self.0
                    .lock()
                    .alloc(layout)
                    .ok()
                    .map_or(core::ptr::null_mut::<u8>(), |allocation| allocation.as_ptr())
            }

            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                self.0
                    .lock()
                    .dealloc(unsafe { NonNull::new_unchecked(ptr) }, layout)
            }
        }
    } else {
        pub fn init() {}
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
        // 示意，这个常量已经没了
        // pub const PHYSICAL_MEMORY_OFFSET: usize = KERNEL_OFFSET - PHYS_MEMORY_BASE;
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
