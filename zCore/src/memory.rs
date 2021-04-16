//! Define the FrameAllocator for physical memory
//! x86_64      --  64GB

use {
    bitmap_allocator::BitAlloc,
    buddy_system_allocator::LockedHeap,
    spin::Mutex,
};
use crate::consts::{PHYSICAL_MEMORY_OFFSET, KERNEL_OFFSET, KERNEL_HEAP_SIZE, MEMORY_OFFSET, MEMORY_END};
use core::alloc::Layout;
use core::ptr::NonNull;
use core::mem;

use kernel_hal_bare::paging::PageTableImpl;
use rcore_memory::memory_set::{MemoryAttr, handler::Linear};

pub type MemorySet = rcore_memory::memory_set::MemorySet<PageTableImpl>;

#[cfg(target_arch = "x86_64")]
use {
    rboot::{BootInfo, MemoryType},
    x86_64::structures::paging::page_table::{PageTable, PageTableFlags as EF},
};

#[cfg(target_arch = "riscv64")]
use riscv::{addr::Frame,
            paging::{
                PageTable, PageTableEntry, PageTableFlags as EF
            }};

#[cfg(target_arch = "x86_64")]
type FrameAlloc = bitmap_allocator::BitAlloc16M;

#[cfg(target_arch = "riscv64")]
type FrameAlloc = bitmap_allocator::BitAlloc1M;

static FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::DEFAULT);

/*
const MEMORY_OFFSET: usize = 0;
const KERNEL_OFFSET: usize = 0xffffff00_00000000;
const PHYSICAL_MEMORY_OFFSET: usize = 0xffff8000_00000000;

#[cfg(target_arch = "x86_64")]
const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB

#[cfg(target_arch = "riscv64")]
const KERNEL_HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MB
*/

const KERNEL_PM4: usize = (KERNEL_OFFSET >> 39) & 0o777;
const PHYSICAL_MEMORY_PM4: usize = (PHYSICAL_MEMORY_OFFSET >> 39) & 0o777;

const PAGE_SIZE: usize = 1 << 12;

#[used]
#[export_name = "hal_pmem_base"]
static PMEM_BASE: usize = PHYSICAL_MEMORY_OFFSET;

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "riscv64")]
use kernel_hal_bare::BootInfo;

#[cfg(target_arch = "riscv64")]
pub fn init_frame_allocator(boot_info: &BootInfo) {
    use bitmap_allocator::BitAlloc;
    use core::ops::Range;

    let mut ba = FRAME_ALLOCATOR.lock();
    let range = to_range(
        (end as usize) - KERNEL_OFFSET + MEMORY_OFFSET + PAGE_SIZE,
        MEMORY_END,
    );
    ba.insert(range);

    info!("frame allocator: init end");

    /// Transform memory area `[start, end)` to integer range for `FrameAllocator`
    fn to_range(start: usize, end: usize) -> Range<usize> {
        let page_start = (start - MEMORY_OFFSET) / PAGE_SIZE;
        let page_end = (end - MEMORY_OFFSET - 1) / PAGE_SIZE + 1;
        assert!(page_start < page_end, "illegal range for frame allocator");
        page_start..page_end
    }
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
pub extern "C" fn hal_heap_alloc(size: &usize, align: &usize) -> usize {
    let ret = HEAP_ALLOCATOR
        .lock()
        .alloc(Layout::from_size_align(*size, *align).unwrap()).unwrap().as_ptr();

    trace!("Allocate heap: {:x?}", ret);
    ret as usize
}

#[no_mangle]
pub extern "C" fn hal_heap_dealloc(ptr: &usize, size: &usize, align: &usize) {
    trace!("Deallocate heap: {:x}", *ptr);
    HEAP_ALLOCATOR
        .lock()
        .dealloc(NonNull::new(*ptr as *mut u8).unwrap(), Layout::from_size_align(*size, *align).unwrap());
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn frame_alloc() -> Option<usize> {
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
pub extern "C" fn frame_dealloc(target: &usize) {
    trace!("Deallocate frame: {:x}", *target);
    FRAME_ALLOCATOR
        .lock()
        .dealloc((*target - MEMORY_OFFSET) / PAGE_SIZE);
}

#[no_mangle]
#[cfg(target_arch = "x86_64")]
pub extern "C" fn hal_pt_map_kernel(pt: &mut PageTable, current: &PageTable) {
    let ekernel = current[KERNEL_PM4].clone();
    let ephysical = current[PHYSICAL_MEMORY_PM4].clone();
    pt[KERNEL_PM4].set_addr(ekernel.addr(), ekernel.flags() | EF::GLOBAL);
    pt[PHYSICAL_MEMORY_PM4].set_addr(ephysical.addr(), ephysical.flags() | EF::GLOBAL);
}

#[no_mangle]
#[cfg(target_arch = "riscv64")]
pub extern "C" fn hal_pt_map_kernel(pt: &mut PageTable, current: &PageTable) {
    //warn!("hal_pt_map_kernel() is NULL! Please use paging::PageTableImpl::map_kernel()");
    //用新页表映射整个kernel; 一般在创建一个新页表时,如PageTableExt中

    debug!("new hal_pt_map_kernel()");
    let ekernel = current[KERNEL_PM4].clone();
    let ephysical = current[PHYSICAL_MEMORY_PM4].clone();
    pt[KERNEL_PM4].set(Frame::of_addr(ekernel.addr()), ekernel.flags() | EF::GLOBAL);
    pt[PHYSICAL_MEMORY_PM4].set(Frame::of_addr(ephysical.addr()), ephysical.flags() | EF::GLOBAL);
}

pub fn remap_the_kernel(dtb: usize) {
    //let mut ms = MemorySet::new();

    //这里如需多级页表映射，就不能用MemorySet::new(), 因为它会先映射1G大页，影响后面的多级页表映射
    let mut ms = MemorySet::new_bare();

    let offset = -(PHYSICAL_MEMORY_OFFSET as isize);
    debug!("remap kernel page:{:#x} -> frame:{:#x}", stext as usize, stext as isize + offset);
    ms.push(
        stext as usize,
        etext as usize,
        MemoryAttr::default().execute().readonly(),
        Linear::new(offset),
        "text",
    );
    ms.push(
        sdata as usize,
        edata as usize,
        MemoryAttr::default(),
        Linear::new(offset),
        "data",
    );
    ms.push(
        srodata as usize,
        erodata as usize,
        MemoryAttr::default().readonly(),
        Linear::new(offset),
        "rodata",
    );
    ms.push(
        bootstack as usize,
        bootstacktop as usize,
        MemoryAttr::default(),
        Linear::new(offset),
        "stack",
    );
    ms.push(
        sbss as usize,
        ebss as usize,
        MemoryAttr::default(),
        Linear::new(offset),
        "bss",
    );

    //堆空间也映射前面一点
    ms.push(
        end as usize + PAGE_SIZE,
        end as usize + PAGE_SIZE*4096,
        MemoryAttr::default(),
        Linear::new(offset),
        "heap",
    );
    debug!("Map heap page: {:#x} ~ {:#x} --> frame: {:#x} ~ {:#x}", end as usize + PAGE_SIZE, end as usize + PAGE_SIZE*1024, end as isize + 4096 + offset, end as isize + 4096*1024 + offset);

    ms.push(
        dtb,
        dtb + super::consts::MAX_DTB_SIZE,
        MemoryAttr::default().readonly(),
        Linear::new(offset),
        "dts",
    );

    // map PLIC for HiFiveU & VirtIO
    let offset = -(KERNEL_OFFSET as isize);
    ms.push(
        KERNEL_OFFSET + 0x0C00_0000,
        KERNEL_OFFSET + 0x0C00_0000 + PAGE_SIZE*4,
        MemoryAttr::default(),
        Linear::new(offset),
        "plic_priority",
    );
    ms.push(
        KERNEL_OFFSET + 0x0C20_0000,
        KERNEL_OFFSET + 0x0C20_0000 + PAGE_SIZE*4,
        MemoryAttr::default(),
        Linear::new(offset),
        "plic_threshold",
    );
    // map UART for HiFiveU
    ms.push(
        KERNEL_OFFSET + 0x10010000,
        KERNEL_OFFSET + 0x10010000 + PAGE_SIZE,
        MemoryAttr::default(),
        Linear::new(offset),
        "uart",
    );
    // map UART for VirtIO
    ms.push(
        KERNEL_OFFSET + 0x10000000,
        KERNEL_OFFSET + 0x10000000 + PAGE_SIZE,
        MemoryAttr::default(),
        Linear::new(offset),
        "uart16550",
    );

    //最后写satp
    unsafe {
        ms.activate();
    }
    unsafe {
        SATP = ms.token();
    }
    mem::forget(ms);
    info!("remap kernel end");
}

// First core stores its SATP here.
static mut SATP: usize = 0;

pub unsafe fn clear_bss() {
    let start = sbss as usize;
    let end = ebss as usize;
    let step = core::mem::size_of::<usize>();
    for i in (start..end).step_by(step) {
        (i as *mut usize).write(0);
    }
}

#[inline]
pub const fn phys_to_virt(paddr: usize) -> usize {
    PHYSICAL_MEMORY_OFFSET + paddr
}

#[inline]
pub const fn virt_to_phys(vaddr: usize) -> usize {
    vaddr - PHYSICAL_MEMORY_OFFSET
}

#[inline]
pub const fn kernel_offset(vaddr: usize) -> usize {
    vaddr - KERNEL_OFFSET
}

//测试:只读权限，却要写入
pub fn write_readonly_test() {
    debug!("rodata write !");
    unsafe {
        let ptr = srodata as usize as *mut u8;
        *ptr = 0xab;
    }
}

//测试:不允许执行，非要执行
pub fn execute_unexecutable_test() {
    debug!("bss execute !");
    unsafe {
        llvm_asm!("jr $0" :: "r"(sbss as usize) :: "volatile");
    }
}

//测试:找不到页表项
pub fn read_invalid_test() {
    debug!("invalid page read !");
    println!("{}", unsafe { *(0x12345678 as usize as *const u8) });
}

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
