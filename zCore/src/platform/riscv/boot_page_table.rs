use super::consts::{kernel_mem_info, kernel_mem_probe};
use core::arch::asm;
use page_table::{MmuFlags, PageTable, Sv39, VAddr, OFFSET_BITS, PPN};

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
        // 内核 GiB 页表项
        let pte = DAGXWRV.build_pte(PPN(mem_info.paddr_base >> OFFSET_BITS));
        // 映射内核页和跳板页
        self.0.set_entry(VAddr(mem_info.paddr_base), pte, 2);
        self.0.set_entry(VAddr(mem_info.vaddr_base), pte, 2);
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
        satp::set(
            satp::Mode::Sv39,
            0,
            self as *const _ as usize >> OFFSET_BITS,
        );
        // 此时原本的地址空间还在，所以不用刷快表
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
    ///
    /// 导致栈重定位，栈上的指针将失效！
    #[naked]
    unsafe extern "C" fn jump_higher(offset: usize) {
        asm!("add sp, sp, a0", "add ra, ra, a0", "ret", options(noreturn))
    }
}
