use super::{
    boot_page_table::BootPageTable,
    consts::{MAX_HART_NUM, PHYSICAL_MEMORY_OFFSET, STACK_PAGES_PER_HART},
};
use core::arch::asm;
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
    // 使能启动页表
    let sstatus = unsafe {
        BOOT_PAGE_TABLE.init();
        BOOT_PAGE_TABLE.launch(hartid)
    };

    println!(
        "
boot page table launched, sstatus = {sstatus:#x}
parse device tree from {device_tree_paddr:#x}
"
    );

    // 启动副核
    let smp = parse_smp(device_tree_paddr);
    println!("smp = {smp}");
    for id in 0..smp {
        if id != hartid {
            println!("hart{id} is booting...");
            let err_code = hart_start(
                id,
                secondary_hart_start as usize - PHYSICAL_MEMORY_OFFSET,
                0,
            );
            if err_code != SBI_SUCCESS {
                panic!("start hart{id} failed. error code={err_code}");
            }
            let hart_mask = 1usize << id;
            let err_code = send_ipi(&hart_mask as *const _ as _);
            if err_code != SBI_SUCCESS {
                panic!("send ipi to hart{id} failed. error code={err_code}");
            }
        } else {
            println!("hart{id} is the primary hart.");
        }
    }
    println!();

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
        "   bnez t0, 1b",
        "   ret",
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

fn parse_smp(dtb_pa: usize) -> usize {
    use dtb_walker::{Dtb, DtbObj, WalkOperation::*};

    let mut smp = 0usize;
    unsafe { Dtb::from_raw_parts(dtb_pa as _) }
        .unwrap()
        .walk(|path, obj| match obj {
            DtbObj::SubNode { name } => {
                if path.last().is_empty() {
                    // 只关心 cpus 节点
                    if name == b"cpus" {
                        StepInto
                    } else if smp > 0 {
                        Terminate
                    } else {
                        StepOver
                    }
                } else {
                    if path.last() == b"cpus" && name.starts_with(b"cpu@") {
                        smp += 1;
                    }
                    StepOver
                }
            }
            DtbObj::Property(_) => StepOver,
        });
    smp
}
