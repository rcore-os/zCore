//! Define physical frame allocation and dynamic memory allocation.

use crate::platform::phys_to_virt_offset;
use alloc::alloc::handle_alloc_error;
use core::{
    alloc::{GlobalAlloc, Layout},
    num::NonZeroUsize,
    ops::Range,
    ptr::NonNull,
};
use customizable_buddy::{BuddyAllocator, LinkedListBuddy, UsizeBuddy};
use kernel_hal::PhysAddr;
use lock::Mutex;

/// 标准分配器类型。
type MutAllocator<const N: usize> = BuddyAllocator<N, UsizeBuddy, LinkedListBuddy>;

/// 堆分配器。
static HEAP: Mutex<MutAllocator<27>> = Mutex::new(MutAllocator::new()); // 27 + 6 + 3  = 36 -> 16 GiB

/// 单页地址位数。
const PAGE_BITS: usize = 12;

/// 为启动准备的初始页数。
const BOOT_PAGES: usize = 512;

pub fn frame_alloc(frame_count: usize, align_log2: usize) -> Option<PhysAddr> {
    let (ptr, size) = HEAP
        .lock()
        .allocate::<u8>(align_log2 << PAGE_BITS, unsafe {
            NonZeroUsize::new_unchecked(frame_count << PAGE_BITS)
        })
        .ok()?;
    assert_eq!(size, frame_count << PAGE_BITS);
    Some(ptr.as_ptr() as PhysAddr - phys_to_virt_offset())
}

pub fn frame_dealloc(target: PhysAddr) {
    HEAP.lock().deallocate(
        unsafe { NonNull::new_unchecked((target + phys_to_virt_offset()) as *mut u8) },
        1 << PAGE_BITS,
    );
}

pub fn init() {
    /// 4 KiB 页类型。
    #[repr(C, align(4096))]
    pub struct Memory<const N: usize>([[u8; 4096]; N]);

    /// 托管空间 2 MiB
    static mut MEMORY: Memory<BOOT_PAGES> = Memory([[0u8; 4096]; BOOT_PAGES]);
    unsafe {
        let ptr = NonNull::new(MEMORY.0.as_mut_ptr()).unwrap();
        log::info!(
            "MEMORY = {:#x}..{:#x}",
            ptr.as_ptr() as usize,
            ptr.as_ptr() as usize + (BOOT_PAGES << PAGE_BITS)
        );

        let mut heap = HEAP.lock();
        heap.init(core::mem::size_of::<usize>().trailing_zeros() as _, ptr);
        heap.transfer(ptr, BOOT_PAGES << PAGE_BITS);
    }
}

pub fn insert_regions(regions: &[Range<PhysAddr>]) {
    let offset = phys_to_virt_offset();
    regions
        .iter()
        .filter(|region| !region.is_empty())
        .for_each(|region| unsafe {
            HEAP.lock().transfer(
                NonNull::new_unchecked((region.start + offset) as *mut u8),
                region.len(),
            );
        });
}

struct Global;

#[global_allocator]
static GLOBAL: Global = Global;

unsafe impl GlobalAlloc for Global {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if let Ok((ptr, _)) = HEAP.lock().allocate_layout::<u8>(layout) {
            ptr.as_ptr()
        } else {
            handle_alloc_error(layout)
        }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HEAP.lock()
            .deallocate_layout(NonNull::new(ptr).unwrap(), layout)
    }
}
