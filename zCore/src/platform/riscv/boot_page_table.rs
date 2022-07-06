use super::consts::{kernel_mem_info, kernel_mem_probe};
use core::arch::asm;
use page_table::{MmuFlags, PageTable, Sv39, OFFSET_BITS, PPN};

/// 启动页表。
pub(super) struct BootPageTable {
    root: PageTable<Sv39>,
    sub: PageTable<Sv39>,
}

/// 内核页属性
const KERNEL_PAGE: MmuFlags<Sv39> = MmuFlags::new(0xef); // DAG_'XWRV

/// 子页表属性
const SUBTABLE: MmuFlags<Sv39> = MmuFlags::new(0x21); // __G_'___V

impl BootPageTable {
    /// 初始化为全零的启动页表。
    pub const ZERO: Self = Self {
        root: PageTable::ZERO,
        sub: PageTable::ZERO,
    };

    /// 根据内核实际位置初始化启动页表。
    pub fn init(&mut self) {
        // 启动页表初始化之前 pc 必定在物理地址空间
        // 因此可以安全地定位内核地址信息
        let mem_info = unsafe { kernel_mem_probe() };
        let pbase = mem_info.paddr_base;
        let vbase = mem_info.vaddr_base;

        const GIB_MASK: usize = !((1 << 30) - 1);
        const SIZE_2MIB: usize = 1 << 21;
        const MASK_2MIB: usize = !(SIZE_2MIB - 1);
        {
            // 把内核起始位置到其所在 GiB 页的末尾映射到虚拟地址空间
            let mut p = (pbase & MASK_2MIB)..((pbase & GIB_MASK) + (1 << 30));
            let mut v = vbase & MASK_2MIB;
            while !p.is_empty() {
                let entry = KERNEL_PAGE.build_pte(PPN(p.start >> OFFSET_BITS));
                self.sub.set_entry(v.into(), entry, 1).unwrap();
                p.start += SIZE_2MIB;
                v += SIZE_2MIB;
            }
        }
        // 映射跳板页和内核页
        let raw = KERNEL_PAGE.build_pte(PPN((pbase & GIB_MASK) >> OFFSET_BITS));
        let sub = SUBTABLE.build_pte(PPN(self.sub.as_ptr() as usize >> OFFSET_BITS));
        self.root
            .set_entry((pbase & GIB_MASK).into(), raw, 2)
            .unwrap();
        self.root
            .set_entry((vbase & GIB_MASK).into(), sub, 2)
            .unwrap();
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
            self.root.as_ptr() as usize >> OFFSET_BITS,
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
