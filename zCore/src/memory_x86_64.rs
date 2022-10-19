//! Define physical frame allocation and dynamic memory allocation.

use bitmap_allocator::BitAlloc;
use core::ops::Range;
use kernel_hal::PhysAddr;
use lock::Mutex;

type FrameAlloc = bitmap_allocator::BitAlloc16M; // max 64G

const PAGE_BITS: usize = 12;

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
    for region in regions {
        let frame_start = phys_addr_to_frame_idx(region.start);
        let frame_end = phys_addr_to_frame_idx(region.end - 1) + 1;
        if frame_start < frame_end {
            ba.insert(frame_start..frame_end);
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
                self.0.lock().dealloc(NonNull::new_unchecked(ptr), layout)
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
