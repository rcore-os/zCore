//! Define dynamic memory allocation.

use crate::platform::phys_to_virt_offset;
use alloc::alloc::handle_alloc_error;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{
    alloc::{GlobalAlloc, Layout},
    num::NonZeroUsize,
    ops::Range,
    ptr::NonNull,
};
use customizable_buddy::{BuddyAllocator, LinkedListBuddy, UsizeBuddy};
use kernel_hal::PhysAddr;
use lock::Mutex;

static TOTAL_MEMORY: AtomicUsize = AtomicUsize::new(0);
static USED_MEMORY: AtomicUsize = AtomicUsize::new(0);

/// 堆分配器。
///
/// 27 + 6 + 3 = 36 -> 64 GiB
struct LockedHeap(Mutex<BuddyAllocator<27, UsizeBuddy, LinkedListBuddy>>);

#[global_allocator]
static HEAP: LockedHeap = LockedHeap(Mutex::new(BuddyAllocator::new()));

/// 单页地址位数。
const PAGE_BITS: usize = 12;

/// 为启动准备的初始内存。
///
/// 经测试，不同硬件的需求：
///
/// | machine         | memory
/// | --------------- | -
/// | qemu,virt SMP 1 |  16 KiB
/// | qemu,virt SMP 4 |  32 KiB
/// | allwinner,nezha | 256 KiB
const MEMORY_SIZE: usize = 2 * 1024 * 1024;
static mut MEMORY: [u8; MEMORY_SIZE] = [0u8; MEMORY_SIZE];

unsafe impl GlobalAlloc for LockedHeap {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if let Ok((ptr, size)) = self.0.lock().allocate_layout(layout) {
            USED_MEMORY.fetch_add(size, Ordering::Relaxed);
            ptr.as_ptr()
        } else {
            handle_alloc_error(layout)
        }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        USED_MEMORY.fetch_sub(layout.size(), Ordering::Relaxed);
        self.0
            .lock()
            .deallocate_layout(NonNull::new(ptr).unwrap(), layout)
    }
}

/// 初始化分配器，并将一个小的内存块注册到分配器中，用于启动需要的动态内存。
pub fn init() {
    unsafe {
        // Use a raw pointer to the static (no `&`/`&mut` to a `static mut`).
        let base = core::ptr::addr_of_mut!(MEMORY) as *mut u8;
        log::info!(
            "MEMORY = {:#x}..{:#x}",
            base as usize,
            base as usize + MEMORY_SIZE
        );
        let mut heap = HEAP.0.lock();
        let ptr = NonNull::new(base).unwrap();
        heap.init(core::mem::size_of::<usize>().trailing_zeros() as _, ptr);
        heap.transfer(ptr, MEMORY_SIZE);
        TOTAL_MEMORY.fetch_add(MEMORY_SIZE, Ordering::Relaxed);
    }
}

/// 将一些内存区域注册到分配器。
pub fn insert_regions(regions: &[Range<PhysAddr>]) {
    let mut heap = HEAP.0.lock();
    let offset = phys_to_virt_offset();
    regions
        .iter()
        .filter(|region| !region.is_empty())
        .for_each(|region| unsafe {
            heap.transfer(
                NonNull::new_unchecked((region.start + offset) as *mut u8),
                region.len(),
            );
            TOTAL_MEMORY.fetch_add(region.len(), Ordering::Relaxed);
        });
}

pub fn frame_alloc(frame_count: usize, align_log2: usize) -> Option<PhysAddr> {
    let (ptr, size) = HEAP
        .0
        .lock()
        .allocate::<u8>(align_log2 << PAGE_BITS, unsafe {
            NonZeroUsize::new_unchecked(frame_count << PAGE_BITS)
        })
        .ok()?;
    assert_eq!(size, frame_count << PAGE_BITS);
    // [diag] These frames back userspace VMOs, and they come out of the SAME
    // buddy arena as the kernel's coroutine stacks (the `GlobalAlloc` impl
    // above locks this very heap). Handing out a block that is already a live
    // stack would reproduce docs/README-crash-repro.md exactly: the VMO is
    // zero-filled on creation, blanking a live kernel stack (`rip=0x0`,
    // `[rsp0..3]=0`, `region=usable` — an overflow would have faulted in the
    // unmapped guard instead), and userspace writing its buffer afterwards
    // sprays arbitrary bytes over kernel stacks and heap objects.
    //
    // Checked here rather than via a canary or watchpoint because this fires
    // at hand-out — naming the aliasing BEFORE anything is corrupted. The
    // registry is plain atomics, so the cost is a short scan of relaxed loads
    // (the heap lock above is already released by this point).
    frame_alias_check(ptr.as_ptr() as usize, size);
    USED_MEMORY.fetch_add(size, Ordering::Relaxed);
    Some(ptr.as_ptr() as PhysAddr - phys_to_virt_offset())
}

/// [diag] Panic loudly if a just-allocated frame range overlaps a live kernel
/// coroutine stack. A no-op on libos, where the scheduler's stacks are not
/// carved out of this heap.
#[cfg(not(feature = "libos"))]
fn frame_alias_check(vaddr: usize, size: usize) {
    if let Some(stack_base) = executor::overlapping_live_stack(vaddr, size) {
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "\n[frame-alias] ALLOCATOR HANDED OUT A LIVE KERNEL STACK\n\
             [frame-alias]   frames  {:#x}..{:#x} ({} bytes)\n\
             [frame-alias]   overlaps coroutine stack alloc_base={:#x}\n\
             [frame-alias] this block is about to back a userspace VMO: the\n\
             [frame-alias] zero-fill alone will blank a live kernel stack.\n",
            vaddr,
            vaddr + size,
            size,
            stack_base,
        ));
        panic!(
            "frame_alloc aliased a live coroutine stack: frames {:#x}..{:#x} vs stack {:#x}",
            vaddr,
            vaddr + size,
            stack_base
        );
    }
}

#[cfg(feature = "libos")]
fn frame_alias_check(_vaddr: usize, _size: usize) {}

pub fn frame_dealloc(target: PhysAddr) {
    USED_MEMORY.fetch_sub(1 << PAGE_BITS, Ordering::Relaxed);
    HEAP.0.lock().deallocate(
        unsafe { NonNull::new_unchecked((target + phys_to_virt_offset()) as *mut u8) },
        1 << PAGE_BITS,
    );
}

pub fn stats() -> (usize, usize) {
    (
        USED_MEMORY.load(Ordering::Relaxed),
        TOTAL_MEMORY.load(Ordering::Relaxed),
    )
}

/// Bytes of heap currently in use (mirrors `memory_x86_64::heap_used`).
#[allow(dead_code)]
pub fn heap_used() -> usize {
    USED_MEMORY.load(Ordering::Relaxed)
}

/// Total bytes managed by the heap (mirrors `memory_x86_64::heap_total`).
#[allow(dead_code)]
pub fn heap_total() -> usize {
    TOTAL_MEMORY.load(Ordering::Relaxed)
}
