use super::consts::{kernel_mem_info, kernel_mem_probe};
use core::arch::asm;
use page_table::{MmuFlags, PageTable, Sv39, PPN};

/// 启动页表。
#[repr(align(4096))]
pub(super) struct BootPageTable(PageTable<Sv39>);

/// 内核页属性
const DAGXWRV: MmuFlags<Sv39> = MmuFlags::new(0xef);

impl BootPageTable {
    /// 初始化为全零的启动页表。
    pub const ZERO: Self = Self(PageTable::ZERO);

    /// 根据内核实际位置初始化启动页表。
    pub fn init(&mut self) {
        // 启动页表初始化之前 pc 必定在物理地址空间
        // 因此可以安全地定位内核地址信息
        let mem_info = unsafe { kernel_mem_probe() };
        // GiB 物理页帧在 GiB 页表中的序号
        let trampoline_pte_index = mem_info.paddr_base >> bits::GIB;
        // GiB 页物理页号
        let start_ppn = mem_info.paddr_base >> bits::KIB;
        // 映射跳板页
        self.0[trampoline_pte_index] = DAGXWRV.build_pte(PPN(start_ppn));
        // 物理地址 0 映射到内核地址偏移处，并依次映射虚拟地址空间后续所有页
        let mut memory_ppn = 0;
        let mut kernel_vpn = (mem_info.offset() & ((1 << 39) - 1)) >> bits::GIB;
        while kernel_vpn < self.0.len() {
            self.0[kernel_vpn] = DAGXWRV.build_pte(PPN(memory_ppn));
            kernel_vpn += 1;
            memory_ppn += 1 << (bits::GIB - bits::KIB);
        }
    }

    /// 启动地址转换，跃迁到高地址，并设置线程指针和内核对用户页的访问权限。
    ///
    /// # Safety
    ///
    /// 调用前后位于不同的地址空间，必须内联。
    #[inline(always)]
    pub unsafe fn launch(&self, hartid: usize) -> usize {
        use riscv::register::satp;
        // 启动地址转换
        satp::set(satp::Mode::Sv39, 0, self as *const _ as usize >> bits::KIB);
        // 此时原本的地址空间还在，所以按理说不用刷快表
        // riscv::asm::sfence_vma_all();
        // 跳到高页面对应位置
        Self::jump_higher(kernel_mem_info().offset());
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

mod bits {
    // 各级页面容量
    pub const KIB: usize = 12; // 4KiB
    pub const MIB: usize = KIB + 9; // 2MiB
    pub const GIB: usize = MIB + 9; // 1GiB
}
