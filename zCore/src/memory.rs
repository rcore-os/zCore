//! Define the FrameAllocator for physical memory
//! x86_64      --  64GB

use {
    bitmap_allocator::BitAlloc,
    buddy_system_allocator::LockedHeap,
    rboot::{BootInfo, MemoryType},
    spin::Mutex,
    x86_64::structures::paging::page_table::{PageTable, PageTableFlags as EF},
};

#[cfg(target_arch = "x86_64")]
type FrameAlloc = bitmap_allocator::BitAlloc16M;

static FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::DEFAULT);

const MEMORY_OFFSET: usize = 0;
const KERNEL_OFFSET: usize = 0xffffff00_00000000;
const PHYSICAL_MEMORY_OFFSET: usize = 0xffff8000_00000000;
const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB

const KERNEL_PM4: usize = (KERNEL_OFFSET >> 39) & 0o777;
const PHYSICAL_MEMORY_PM4: usize = (PHYSICAL_MEMORY_OFFSET >> 39) & 0o777;

const PAGE_SIZE: usize = 1 << 12;

#[used]
#[export_name = "hal_pmem_base"]
static PMEM_BASE: usize = PHYSICAL_MEMORY_OFFSET;

pub fn init_frame_allocator(boot_info: &BootInfo) {
    let mut ba = FRAME_ALLOCATOR.lock();
    for region in boot_info.memory_map.iter() {
        if region.ty == MemoryType::CONVENTIONAL {
            let start_frame = region.phys_start as usize / PAGE_SIZE;
            let end_frame = start_frame + region.page_count as usize;
            ba.insert(start_frame..end_frame);
        }
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
    info!("heap init end");
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
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
#[allow(improper_ctypes_definitions)]
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

#[no_mangle]
pub extern "C" fn hal_pt_map_kernel(pt: &mut PageTable, current: &PageTable) {
    let ekernel = current[KERNEL_PM4].clone();
    let ephysical = current[PHYSICAL_MEMORY_PM4].clone();
    pt[KERNEL_PM4].set_addr(ekernel.addr(), ekernel.flags() | EF::GLOBAL);
    pt[PHYSICAL_MEMORY_PM4].set_addr(ephysical.addr(), ephysical.flags() | EF::GLOBAL);
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
