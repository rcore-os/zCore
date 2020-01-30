//! Define the FrameAllocator for physical memory
//! x86_64      --  64GB

use {
    super::HEAP_ALLOCATOR,
    bitmap_allocator::BitAlloc,
    buddy_system_allocator::Heap,
    core::mem,
    lazy_static::*,
    rboot::{BootInfo, MemoryType},
    spin::Mutex,
};

// x86_64 support up to 1T memory
#[cfg(target_arch = "x86_64")]
pub type FrameAlloc = bitmap_allocator::BitAlloc256M;

lazy_static! {
    pub static ref FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::default());
}

const KERNEL_HEAP_SIZE: usize = 8 * 1024 * 1024;
const PAGE_SIZE: usize = 1 << 12;
const MEMORY_OFFSET: usize = 0;

pub fn init_frame_allocator(boot_info: &BootInfo) {
    let mut ba = FRAME_ALLOCATOR.lock();
    for region in boot_info.memory_map.clone().iter {
        if region.ty == MemoryType::CONVENTIONAL {
            let start_frame = region.phys_start as usize / PAGE_SIZE;
            let end_frame = start_frame + region.page_count as usize;
            ba.insert(start_frame..end_frame);
        }
    }
    info!("Frame allocator init end");
}

pub fn init_heap() {
    const MACHINE_ALIGN: usize = mem::size_of::<usize>();
    const HEAP_BLOCK: usize = KERNEL_HEAP_SIZE / MACHINE_ALIGN;
    static mut HEAP: [usize; HEAP_BLOCK] = [0; HEAP_BLOCK];
    unsafe {
        HEAP_ALLOCATOR
            .lock()
            .init(HEAP.as_ptr() as usize, HEAP_BLOCK * MACHINE_ALIGN);
    }
    info!("heap init end");
}

pub fn alloc_frame() -> Option<usize> {
    // get the real address of the alloc frame
    let ret = FRAME_ALLOCATOR
        .lock()
        .alloc()
        .map(|id| id * PAGE_SIZE + MEMORY_OFFSET);
    trace!("Allocate frame: {:x?}", ret);
    ret
}
pub fn dealloc_frame(target: usize) {
    trace!("Deallocate frame: {:x}", target);
    FRAME_ALLOCATOR
        .lock()
        .dealloc((target - MEMORY_OFFSET) / PAGE_SIZE);
}

pub fn enlarge_heap(heap: &mut Heap) {
    info!("Enlarging heap to avoid oom");

    let mut addrs = [(0, 0); 32];
    let mut addr_len = 0;
    let va_offset = MEMORY_OFFSET;
    for i in 0..16384 {
        let page = alloc_frame().unwrap();
        let va = va_offset + page;
        if addr_len > 0 {
            let (ref mut addr, ref mut len) = addrs[addr_len - 1];
            if *addr - PAGE_SIZE == va {
                *len += PAGE_SIZE;
                *addr -= PAGE_SIZE;
                continue;
            }
        }
        addrs[addr_len] = (va, PAGE_SIZE);
        addr_len += 1;
    }
    for (addr, len) in addrs[..addr_len].into_iter() {
        info!("Adding {:#X} {:#X} to heap", addr, len);
        unsafe {
            heap.init(*addr, *len);
        }
    }
}
