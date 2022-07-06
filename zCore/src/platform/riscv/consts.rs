// RISCV

/// 内核堆容量。
pub const KERNEL_HEAP_SIZE: usize = 80 * 1024 * 1024; // 80 MB

/// 内核每个硬件线程的栈页数。
pub const STACK_PAGES_PER_HART: usize = 32;

/// 最大的对称多核硬件线程数量。
pub const MAX_HART_NUM: usize = 8;

#[inline]
pub fn phys_memory_base() -> usize {
    kernel_mem_info().paddr_base
}

use spin::Once;

/// 内核位置信息
pub struct KernelMemInfo {
    /// 内核所在物理 GiB 页的起始地址。
    ///
    /// 这个地址也被视物理地址空间中主存的起始地址。
    pub paddr_base: usize,

    /// 内核所在虚拟 GiB 页的起始地址。
    ///
    /// 实际上是 Sv39 方案虚存的最后一个 GiB 页的起始地址。
    pub vaddr_base: usize,

    /// 内核链接区域长度。
    pub size: usize,
}

impl KernelMemInfo {
    /// 初始化物理内存信息。
    ///
    /// # Safety
    ///
    /// 这个函数必须在 `pc` 仍在物理地址空间时调用！
    ///
    /// 除此之外，其正确性还依赖下列条件：
    ///
    /// 1. 主存的物理地址对齐到 1 GiB
    /// 3. 内核的链接地址与内核的物理地址在一个 GiB 内的偏移一致
    unsafe fn new() -> Self {
        // 这个函数必须在 pc 仍在物理地址上时调用！
        extern "C" {
            fn start();
            fn end();
        }
        const GIB_MASK: usize = !((1 << 30) - 1);
        let paddr_base = GIB_MASK & (start as usize);
        let vaddr_base = GIB_MASK & (!0);
        Self {
            paddr_base,
            vaddr_base,
            size: end as usize - paddr_base,
        }
    }

    /// 计算内核虚存空间到物理地址空间的偏移。
    pub fn offset(&self) -> usize {
        self.vaddr_base - self.paddr_base
    }
}

static KERNEL_MEM_INFO: Once<KernelMemInfo> = Once::new();

#[inline]
pub fn kernel_mem_info() -> &'static KernelMemInfo {
    KERNEL_MEM_INFO.wait()
}

#[inline]
pub(super) unsafe fn kernel_mem_probe() -> &'static KernelMemInfo {
    KERNEL_MEM_INFO.call_once(|| KernelMemInfo::new())
}
