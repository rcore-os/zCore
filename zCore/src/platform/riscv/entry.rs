use super::consts::PHYSICAL_MEMORY_OFFSET;
use core::{
    arch::{asm, global_asm},
    str::FromStr,
};
use kernel_hal::{
    sbi::{hart_start, send_ipi, shutdown, SBI_SUCCESS},
    KernelConfig,
};

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

/// 内核入口。
///
/// # Safety
///
/// 裸函数。
#[naked]
#[no_mangle]
#[link_section = ".text.entry"]
unsafe extern "C" fn _start(hartid: usize, device_tree_paddr: usize) -> ! {
    asm!(
        "csrw sie, zero",      // 关中断
        "call {select_stack}", // 设置启动栈
        "j    {main}",         // 进入 rust
        select_stack = sym select_stack,
        main = sym primary_rust_main,
        options(noreturn)
    )
}

/// 副核入口。此前副核被 SBI 阻塞。
///
/// # Safety
///
/// 裸函数。
#[naked]
unsafe extern "C" fn _secondary_hart_start(hartid: usize) -> ! {
    asm!(
        "csrw sie, zero",      // 关中断
        "call {select_stack}", // 设置启动栈
        "j    {main}",         // 进入 rust
        select_stack = sym select_stack,
        main = sym secondary_rust_main,
        options(noreturn)
    )
}

// 启动栈空间会在 kernel-hal 中重映射，因此必须导出符号
global_asm!(
    "\
    .section .bss.bootstack
    .align 12
    .global bootstack
bootstack:
    .space 4096 * 16 * 10
    .global bootstacktop
bootstacktop:"
);

/// 根据硬件线程号设置启动栈。
///
/// # Safety
///
/// 裸函数。
#[naked]
unsafe extern "C" fn select_stack(hartid: usize) {
    extern "C" {
        fn bootstacktop();
    }
    asm!(
        "   mv   t0, a0",
        "   la   sp, {stack_top}",
        "   beqz t0, 2f",
        "   li   t1, -4096 * 16",
        "1: add  sp, sp, t1",
        "   addi t0, t0, -1",
        "   bgtz t0, 1b",
        "2: ret",
        stack_top = sym bootstacktop,
        options(noreturn)
    )
}

/// 主核启动。
extern "C" fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    // 清零 bss 段
    extern "C" {
        fn sbss();
        fn ebss();
    }
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

/// 副核启动。
extern "C" fn secondary_rust_main(hartid: usize) -> ! {
    let _ = unsafe { BOOT_PAGE_TABLE.launch(hartid) };
    crate::secondary_main()
}

impl BootPageTable {
    /// 向上跳到距离为 `offset` 的新地址然后继续执行。
    ///
    /// # Safety
    ///
    /// 裸函数。
    #[naked]
    unsafe extern "C" fn jump_higher(offset: usize) {
        asm!(
            //
            "add ra, ra, a0",
            "add sp, sp, a0",
            "ret",
            options(noreturn)
        )
    }

    /// 设置启动页表，并跃迁到高地址。
    ///
    /// # Safety
    ///
    /// 调用前后位于不同的地址空间，必须内联。
    #[inline(always)]
    unsafe fn launch(&self, hartid: usize) -> usize {
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
        let sstatus: usize;
        asm!("csrrsi {}, sstatus, 18", out(reg) sstatus);
        sstatus
    }
}
