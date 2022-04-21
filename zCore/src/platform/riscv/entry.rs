use super::consts::PHYSICAL_MEMORY_OFFSET;
use core::{
    arch::{asm, global_asm},
    str::FromStr,
};
use kernel_hal::{
    sbi::{hart_start, send_ipi, shutdown, SBI_SUCCESS},
    KernelConfig,
};

global_asm!(include_str!("boot/entry64.asm"));

// 启动页表
#[repr(align(4096))]
struct BootPageTable([usize; 512]);

static mut BOOT_PAGE_TABLE: BootPageTable = BootPageTable([0; 512]);

// 各级页面容量
const KIB_BITS: usize = 12; // 4KiB
const MIB_BITS: usize = KIB_BITS + 9; // 2MiB
const GIB_BITS: usize = MIB_BITS + 9; // 1GiB

// 各级页号遮罩
// const KIB_MASK: usize = !((1 << KIB_BITS) - 1);
// const MIB_MASK: usize = !((1 << MIB_BITS) - 1);
const GIB_MASK: usize = !((1 << GIB_BITS) - 1);
const SV39_MASK: usize = (1 << (GIB_BITS + 9)) - 1;

/// 填充 `satp`
const MODE_SV39: usize = 8 << 60;

/// 内核页属性
const DAGXWRV: usize = 0xef;

// 符号表
extern "C" {
    /// 内核入口
    fn _start();
    /// 副核入口
    fn _secondary_hart_start();
    /// 向上跳到距离为 `offset` 的新地址，继续执行
    fn _jump_higher(offset: usize);
    /// bss 段起始地址
    fn sbss();
    /// bss 段结束地址
    fn ebss();
}

#[no_mangle]
pub extern "C" fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    // 清零 bss 段
    let len = (ebss as usize - sbss as usize) / core::mem::size_of::<usize>();
    unsafe { core::slice::from_raw_parts_mut(sbss as *mut usize, len) }.fill(0);

    // 内核的 GiB 页物理页号
    let start_ppn = ((_start as usize) & GIB_MASK) >> KIB_BITS;
    // 内核 GiB 物理页帧在 GiB 页表中的序号
    let trampoline_pte_index = (_start as usize) >> GIB_BITS;
    let mut pte_index = (PHYSICAL_MEMORY_OFFSET & SV39_MASK) >> GIB_BITS;
    // 容纳内核的页表项
    let pte = (start_ppn << 10) | DAGXWRV;

    // 构造启动页表
    // # TODO d1 c906 有扩展 63:59 位的页表项属性
    // #.quad (1 << 62) | (1 << 61) | (1 << 60) | (0x40000 << 10) | 0xef
    unsafe {
        *BOOT_PAGE_TABLE.0.get_unchecked_mut(trampoline_pte_index) = pte;
        let mut page = DAGXWRV;
        while pte_index < 512 {
            *BOOT_PAGE_TABLE.0.get_unchecked_mut(pte_index) = page;
            page += 1 << (GIB_BITS + 10 - KIB_BITS);
            pte_index += 1;
        }
    }

    // 启动副核
    for id in 0..usize::from_str(core::env!("SMP")).expect("can't parse SMP as usize.") {
        if id != hartid {
            let err_code = hart_start(id, _secondary_hart_start as _, 0);
            if err_code != SBI_SUCCESS {
                panic!("start hart{} failed. error code={}", id, err_code);
            }
            let hart_mask = 1usize << id;
            let err_code = send_ipi(&hart_mask as *const _ as _);
            if err_code != SBI_SUCCESS {
                panic!("send ipi to hart{} failed. error code={}", id, err_code);
            }
        }
    }

    // 使能启动页表
    let sstatus = unsafe { BOOT_PAGE_TABLE.launch(hartid) };
    println!(
        "
boot hart: zCore rust_main(hartid: {}, device_tree_paddr: {:#x})
sstatus = {:#x}",
        hartid, device_tree_paddr, sstatus
    );

    crate::primary_main(KernelConfig {
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    });
    shutdown()
}

#[no_mangle]
pub extern "C" fn secondary_rust_main(hartid: usize) -> ! {
    let _ = unsafe { BOOT_PAGE_TABLE.launch(hartid) };
    crate::secondary_main()
}

impl BootPageTable {
    /// 设置启动页表，并跃迁到高地址。
    ///
    /// # Safety
    ///
    /// 内含极度危险的地址空间跃迁操作，必须内联。
    #[inline(always)]
    unsafe fn launch(&self, hartid: usize) -> usize {
        // 启动页表的页号，将填写到 `satp`
        let satp = MODE_SV39 | ((self.0.as_ptr() as usize) >> KIB_BITS);
        // 启动地址转换
        riscv::register::satp::write(satp);
        riscv::asm::sfence_vma_all();
        // 跳到高页面对应位置
        _jump_higher(PHYSICAL_MEMORY_OFFSET);
        // 设置线程指针
        asm!("mv tp, {}", in(reg) hartid);
        // 设置内核可访问用户页
        let sstatus: usize;
        asm!("csrrsi {}, sstatus, 18", out(reg) sstatus);
        sstatus
    }
}
