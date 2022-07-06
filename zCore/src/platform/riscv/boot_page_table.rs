#![allow(deprecated)]

use super::consts::{kernel_mem_info, kernel_mem_probe};
use core::arch::asm;
use page_table::{MmuFlags, PageTable, Sv39, OFFSET_BITS, PPN};

/// 启动页表。
#[repr(C, align(4096))]
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
        // 内核 GiB 跳板页
        let raw = KERNEL_PAGE.build_pte(PPN(mem_info.paddr_base >> OFFSET_BITS));
        // MiB 子页表
        let sub = SUBTABLE.build_pte(PPN(self.sub.as_ptr() as usize >> OFFSET_BITS));
        let mut p = mem_info.paddr_base;
        let mut v = mem_info.vaddr_base;
        for _ in 0..self.sub.len() {
            const SIZE_2MIB: usize = 1 << 21;
            let entry = KERNEL_PAGE.build_pte(PPN(p >> OFFSET_BITS));
            self.sub.set_entry(v.into(), entry, 1).unwrap();
            p += SIZE_2MIB;
            v += SIZE_2MIB;
        }
        // 映射跳板页和内核页
        self.root
            .set_entry(mem_info.paddr_base.into(), raw, 2)
            .unwrap();
        self.root
            .set_entry(mem_info.vaddr_base.into(), sub, 2)
            .unwrap();
        sbi_rt::legacy::console_putchar(b'!' as _);
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
        sbi_rt::legacy::console_putchar(b'0' as _);
        satp::set(
            satp::Mode::Sv39,
            0,
            self.root.as_ptr() as usize >> OFFSET_BITS,
        );
        // 此时原本的地址空间还在，所以不用刷快表
        // riscv::asm::sfence_vma_all();
        // 跳到高页面对应位置
        sbi_rt::legacy::console_putchar(b'1' as _);
        Self::jump_higher(kernel_mem_info().offset());
        sbi_rt::legacy::console_putchar(b'2' as _);
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
