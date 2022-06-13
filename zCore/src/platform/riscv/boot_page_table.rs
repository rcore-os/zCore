use super::consts::PHYSICAL_MEMORY_OFFSET;
use consts::*;
use core::arch::asm;

/// 启动页表。
#[repr(align(4096))]
pub(super) struct BootPageTable([usize; 512]);

impl BootPageTable {
    /// 初始化为全零的启动页表。
    pub const ZERO: Self = Self([0; 512]);

    /// 根据内核实际位置初始化启动页表。
    pub fn init(&mut self) {
        extern "C" {
            fn start();
        }
        // 内核起始地址
        let kernel_start = start as usize;
        // GiB 物理页帧在 GiB 页表中的序号
        let trampoline_pte_index = kernel_start >> GIB_BITS;
        // GiB 页物理页号
        let start_ppn = (kernel_start & GIB_MASK) >> KIB_BITS;
        // 映射跳板页
        self.0[trampoline_pte_index] = (start_ppn << 10) | DAGXWRV;
        // 物理地址 0 映射到内核地址偏移处，并依次映射虚拟地址空间后续所有页
        const OFF_PTE: usize = 1 << (GIB_BITS - KIB_BITS + 10);
        const IDX_PTE: usize = (PHYSICAL_MEMORY_OFFSET & SV39_MASK) >> GIB_BITS;
        self.0[IDX_PTE..]
            .iter_mut()
            .enumerate()
            .for_each(|(i, pte)| *pte = (i * OFF_PTE) | DAGXWRV);
    }

    /// 启动地址转换，跃迁到高地址，并设置线程指针和内核对用户页的访问权限。
    ///
    /// # Safety
    ///
    /// 调用前后位于不同的地址空间，必须内联。
    #[inline(always)]
    pub unsafe fn launch(&self, hartid: usize) -> usize {
        // 启动页表的页号，将填写到 `satp`
        let satp = MODE_SV39 | ((self.0.as_ptr() as usize) >> KIB_BITS);
        // 启动地址转换
        riscv::register::satp::write(satp);
        // 此时原本的地址空间还在，所以按理说不用刷快表
        // riscv::asm::sfence_vma_all();
        // 跳到高页面对应位置
        Self::jump_higher(PHYSICAL_MEMORY_OFFSET);
        // 设置线程指针
        asm!("mv tp, {}", in(reg) hartid);
        // 设置内核可访问用户页
        let mut sstatus = 1usize << 18;
        asm!("csrrs {0}, sstatus, {0}", inlateout(reg) sstatus);
        sstatus | (1usize << 18)
    }

    /// 向上跳到距离为 `offset` 的新地址然后继续执行。
    ///
    /// # Safety
    ///
    /// 裸函数。
    #[naked]
    unsafe extern "C" fn jump_higher(offset: usize) {
        asm!("add ra, ra, a0", "add sp, sp, a0", "ret", options(noreturn))
    }
}

#[allow(dead_code)]
mod consts {
    // 各级页面容量
    pub const KIB_BITS: usize = 12; // 4KiB
    pub const MIB_BITS: usize = KIB_BITS + 9; // 2MiB
    pub const GIB_BITS: usize = MIB_BITS + 9; // 1GiB

    // 各级页号遮罩
    pub const KIB_MASK: usize = !((1 << KIB_BITS) - 1);
    pub const MIB_MASK: usize = !((1 << MIB_BITS) - 1);
    pub const GIB_MASK: usize = !((1 << GIB_BITS) - 1);
    pub const SV39_MASK: usize = (1 << (GIB_BITS + 9)) - 1;

    /// 填充 `satp`
    pub const MODE_SV39: usize = 8 << 60;

    /// 内核页属性
    pub const DAGXWRV: usize = 0xef;
}
