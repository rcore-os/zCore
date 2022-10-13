use super::consts::{kernel_mem_info, kernel_mem_probe};
use core::arch::asm;
use page_table::{MmuMeta, Pte, Sv39, VAddr, VmFlags, VmMeta, PPN};

/// 启动页表。
#[repr(align(4096))]
pub(super) struct BootPageTable([Pte<Sv39>; 512]);

impl BootPageTable {
    /// 初始化为全零的启动页表。
    pub const ZERO: Self = Self([Pte::ZERO; 512]);

    /// 根据内核实际位置初始化启动页表。
    pub fn init(&mut self) {
        cfg_if! {
            if #[cfg(feature = "thead-maee")] {
                const FLAGS: VmFlags<Sv39> = unsafe {
                    VmFlags::from_raw(VmFlags::<Sv39>::build_from_str("DAG_XWRV").val() | (1 << 62))
                };
            } else {
                const FLAGS: VmFlags<Sv39> = VmFlags::build_from_str("DAG_XWRV");
            }
        }

        // 启动页表初始化之前 pc 必定在物理地址空间
        // 因此可以安全地定位内核地址信息
        let mem_info = unsafe { kernel_mem_probe() };
        // 确保虚实地址在 1 GiB 内对齐
        assert!(mem_info.offset().trailing_zeros() >= 30);
        // 映射跳板页
        let base = VAddr::<Sv39>::new(mem_info.paddr_base)
            .floor()
            .index_in(Sv39::MAX_LEVEL);
        self.0[base] = FLAGS.build_pte(PPN::new(base << 18));
        // 映射物理地址空间的前 128 GiB
        let base = VAddr::<Sv39>::new(mem_info.offset())
            .floor()
            .index_in(Sv39::MAX_LEVEL);
        for i in 0..128 {
            self.0[base + i] = FLAGS.build_pte(PPN::new(i << 18));
        }
    }

    /// 启动地址转换，跃迁到高地址，并设置线程指针和内核对用户页的访问权限。
    ///
    /// # Safety
    ///
    /// 调用前后位于不同的地址空间，必须内联。
    #[inline(always)]
    pub unsafe fn launch(&self) -> usize {
        use riscv::register::satp;
        // 启动地址转换
        satp::set(
            satp::Mode::Sv39,
            0,
            self.0.as_ptr() as usize >> Sv39::PAGE_BITS,
        );
        // 此时原本的地址空间还在，所以不用刷快表
        // riscv::asm::sfence_vma_all();
        // 跳到高页面对应位置
        Self::jump_higher(kernel_mem_info().offset());
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
