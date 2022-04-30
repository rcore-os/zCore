use super::{boot_page_table::BootPageTable, consts::PHYSICAL_MEMORY_OFFSET};
use core::{arch::asm, str::FromStr};
use kernel_hal::{
    sbi::{hart_start, send_ipi, shutdown, SBI_SUCCESS},
    KernelConfig,
};

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
unsafe extern "C" fn secondary_hart_start(hartid: usize) -> ! {
    asm!(
        "csrw sie, zero",      // 关中断
        "call {select_stack}", // 设置启动栈
        "j    {main}",         // 进入 rust
        select_stack = sym select_stack,
        main = sym secondary_rust_main,
        options(noreturn)
    )
}

/// 启动页表
static mut BOOT_PAGE_TABLE: BootPageTable = BootPageTable::ZERO;

/// 主核启动。
extern "C" fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    // 清零 bss 段
    zero_bss();
    // 初始化启动页表
    unsafe { BOOT_PAGE_TABLE.init() };
    // 启动副核
    for id in 0..usize::from_str(core::env!("SMP")).expect("can't parse SMP as usize.") {
        if id != hartid {
            let err_code = hart_start(id, secondary_hart_start as _, 0);
            if err_code != SBI_SUCCESS {
                panic!("start hart{id} failed. error code={err_code}");
            }
            let hart_mask = 1usize << id;
            let err_code = send_ipi(&hart_mask as *const _ as _);
            if err_code != SBI_SUCCESS {
                panic!("send ipi to hart{id} failed. error code={err_code}");
            }
        }
    }
    // 使能启动页表
    let sstatus = unsafe { BOOT_PAGE_TABLE.launch(hartid) };
    println!(
        "
boot hart: zCore rust_main(hartid: {hartid}, device_tree_paddr: {device_tree_paddr:#x})
sstatus = {sstatus:#x}"
    );
    // 转交控制权
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

/// 根据硬件线程号设置启动栈。
///
/// # Safety
///
/// 裸函数。
#[naked]
unsafe extern "C" fn select_stack(hartid: usize) {
    const STACK_PAGES_PER_HART: usize = 16;
    const MAX_HART_NUM: usize = 10;

    const STACK_LEN_PER_HART: usize = 4096 * STACK_PAGES_PER_HART;
    const STACK_LEN_TOTAL: usize = STACK_LEN_PER_HART * MAX_HART_NUM;

    #[link_section = ".bss.bootstack"]
    static mut BOOT_STACK: [u8; STACK_LEN_TOTAL] = [0u8; STACK_LEN_TOTAL];

    asm!(
        "   addi t0, a0,  1",
        "   la   sp, {stack}",
        "   li   t1, {len_per_hart}",
        "1: add  sp, sp, t1",
        "   addi t0, t0, -1",
        "   bgtz t0, 1b",
        "2: ret",
        stack = sym BOOT_STACK,
        len_per_hart = const STACK_LEN_PER_HART,
        options(noreturn)
    )
}

/// 清零 bss 段
#[inline(always)]
fn zero_bss() {
    #[cfg(target_arch = "riscv32")]
    type Word = u32;
    #[cfg(target_arch = "riscv64")]
    type Word = u64;
    extern "C" {
        static mut sbss: Word;
        static mut ebss: Word;
    }
    unsafe { r0::zero_bss(&mut sbss, &mut ebss) };
}
