//! Define the FrameAllocator for physical memory

use {
    bitmap_allocator::BitAlloc,
    buddy_system_allocator::LockedHeap,
    rboot::{BootInfo, MemoryType},
    spin::Mutex,
};

#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::page_table::{PageTable, PageTableFlags as EF};

#[cfg(target_arch = "mips")]
use mips::paging::PageTable;

// x86_64      --  64GB
#[cfg(target_arch = "x86_64")]
type FrameAlloc = bitmap_allocator::BitAlloc16M;

// RISCV, ARM, MIPS has 1G memory
#[cfg(any(
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "aarch64",
    target_arch = "mips"
))]
pub type FrameAlloc = bitmap_allocator::BitAlloc1M;

static FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::DEFAULT);

#[cfg(target_arch = "x86_64")]
const MEMORY_OFFSET: usize = 0;
#[cfg(target_arch = "x86_64")]
const KERNEL_OFFSET: usize = 0xffffff00_00000000;
#[cfg(target_arch = "x86_64")]
const PHYSICAL_MEMORY_OFFSET: usize = 0xffff8000_00000000;
#[cfg(target_arch = "x86_64")]
const KERNEL_PM4: usize = (KERNEL_OFFSET >> 39) & 0o777;
#[cfg(target_arch = "x86_64")]
const PHYSICAL_MEMORY_PM4: usize = (PHYSICAL_MEMORY_OFFSET >> 39) & 0o777;
#[cfg(target_arch = "x86_64")]
const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB

#[cfg(target_arch = "mips")]
const MEMORY_OFFSET: usize = 0x8000_0000;
#[cfg(target_arch = "mips")]
const KERNEL_OFFSET: usize = 0x8010_0000;
#[cfg(target_arch = "mips")]
const PHYSICAL_MEMORY_OFFSET: usize = 0x8000_0000;
#[cfg(target_arch = "mips")]
const MEMORY_END: usize = 0x8800_0000;
#[cfg(target_arch = "mips")]
const KERNEL_HEAP_SIZE: usize = 0x0200_0000;

const PAGE_SIZE: usize = 1 << 12;

#[used]
#[export_name = "hal_pmem_base"]
static PMEM_BASE: usize = PHYSICAL_MEMORY_OFFSET;

#[cfg(target_arch = "x86_64")]
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

// Symbols provided by linker script
#[cfg(target_arch = "mips")]
#[allow(dead_code)]
extern "C" {
    fn stext();
    fn etext();
    fn sdata();
    fn edata();
    fn srodata();
    fn erodata();
    fn sbss();
    fn ebss();
    fn start();
    fn end();
    fn bootstack();
    fn bootstacktop();
}

#[cfg(target_arch = "mips")]
pub unsafe fn clear_bss() {
    let start = sbss as usize;
    let end = ebss as usize;
    let step = core::mem::size_of::<usize>();
    for i in (start..end).step_by(step) {
        (i as *mut usize).write(0);
    }
}

#[cfg(target_arch = "mips")]
pub fn init_frame_allocator() {
    use core::ops::Range;
    let mut ba = FRAME_ALLOCATOR.lock();
    let range = to_range(
        (end as usize) - KERNEL_OFFSET + MEMORY_OFFSET + PAGE_SIZE,
        MEMORY_END,
    );
    ba.insert(range);

    /// Transform memory area `[start, end)` to integer range for `FrameAllocator`
    fn to_range(start: usize, end: usize) -> Range<usize> {
        info!("frame allocator: start {:#x} end {:#x}", start, end);
        let page_start = (start - MEMORY_OFFSET) / PAGE_SIZE;
        let page_end = (end - MEMORY_OFFSET - 1) / PAGE_SIZE + 1;
        assert!(page_start < page_end, "illegal range for frame allocator");
        page_start..page_end
    }
    info!("Frame allocator init end");
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
}

#[no_mangle]
pub extern "C" fn hal_frame_alloc() -> Option<usize> {
    // get the real address of the alloc frame
    let ret = FRAME_ALLOCATOR
        .lock()
        .alloc()
        .map(|id| id * PAGE_SIZE + MEMORY_OFFSET);
    trace!("Allocate frame: {:x?}", ret);
    ret
}

#[no_mangle]
pub extern "C" fn hal_frame_alloc_contiguous(page_num: usize, align_log2: usize) -> Option<usize> {
    let ret = FRAME_ALLOCATOR
        .lock()
        .alloc_contiguous(page_num, align_log2)
        .map(|id| id * PAGE_SIZE + MEMORY_OFFSET);
    trace!(
        "Allocate contiguous frames: {:x?} ~ {:x?}",
        ret,
        ret.map(|x| x + page_num)
    );
    ret
}

#[no_mangle]
pub extern "C" fn hal_frame_dealloc(target: &usize) {
    trace!("Deallocate frame: {:x}", *target);
    FRAME_ALLOCATOR
        .lock()
        .dealloc((*target - MEMORY_OFFSET) / PAGE_SIZE);
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn hal_pt_map_kernel(pt: &mut PageTable, current: &PageTable) {
    let ekernel = current[KERNEL_PM4].clone();
    let ephysical = current[PHYSICAL_MEMORY_PM4].clone();
    pt[KERNEL_PM4].set_addr(ekernel.addr(), ekernel.flags() | EF::GLOBAL);
    pt[PHYSICAL_MEMORY_PM4].set_addr(ephysical.addr(), ephysical.flags() | EF::GLOBAL);
}

#[cfg(target_arch = "mips")]
#[no_mangle]
pub extern "C" fn hal_pt_map_kernel(_pt: &mut PageTable, _current: &PageTable) {
    // nothing to do
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
    #[rvm::extern_fn(x86_all_traps_handler_addr)]
    unsafe fn rvm_x86_all_traps_handler_addr() -> usize {
        extern "C" {
            fn __alltraps();
        }
        __alltraps as usize
    }
}

/// Global heap allocator
///
/// Available after `memory::init_heap()`.
#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap = LockedHeap::new();
