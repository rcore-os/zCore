use super::{
    boot_page_table::BootPageTable,
    consts::{MAX_HART_NUM, PHYSICAL_MEMORY_OFFSET, STACK_PAGES_PER_HART},
};
use core::arch::asm;
use kernel_hal::KernelConfig;
use sbi_rt as sbi;

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
    // 打印启动信息
    println!(
        "
boot page table launched, sstatus = {sstatus:#x}
parse device tree from {device_tree_paddr:#x}
"
    );
    // 启动副核
    boot_secondary_harts(hartid, device_tree_paddr);
    // 转交控制权
    crate::primary_main(KernelConfig {
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    });
    sbi::system_reset(sbi::RESET_TYPE_SHUTDOWN, sbi::RESET_REASON_NO_REASON);
    unreachable!()
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

// 启动副核
fn boot_secondary_harts(hartid: usize, device_tree_paddr: usize) {
    use dtb_walker::{Dtb, DtbObj, HeaderError, Property, WalkOperation::*};
    let mut cpus = false;
    let mut cpu: Option<usize> = None;
    let dtb = unsafe {
        Dtb::from_raw_parts_filtered(device_tree_paddr as _, |e| {
            matches!(
                e,
                HeaderError::Misaligned(4) | HeaderError::LastCompVersion(16)
            )
        })
    }
    .unwrap();
    dtb.walk(|path, obj| match obj {
        DtbObj::SubNode { name } => {
            if path.last().is_empty() {
                if name == b"cpus" {
                    // 进入 cpus 节点
                    cpus = true;
                    StepInto
                } else if cpus {
                    // 已离开 cpus 节点
                    if let Some(id) = cpu.take() {
                        hart_start(id, hartid);
                    }
                    Terminate
                } else {
                    // 其他节点
                    StepOver
                }
            } else if path.last() == b"cpus" {
                // 如果没有 cpu 序号，肯定是单核的
                if name == b"cpu" {
                    return Terminate;
                }
                if name.starts_with(b"cpu@") {
                    let id: usize = usize::from_str_radix(
                        unsafe { core::str::from_utf8_unchecked(&name[4..]) },
                        16,
                    )
                    .unwrap();
                    if let Some(id) = cpu.replace(id) {
                        hart_start(id, hartid);
                    }
                    StepInto
                } else {
                    StepOver
                }
            } else {
                StepOver
            }
        }
        // 状态不是 "okay" 的 cpu 不能启动
        DtbObj::Property(Property::Status(status))
            if path.last().starts_with(b"cpu@") && status.as_bytes() != b"okay" =>
        {
            if let Some(id) = cpu.take() {
                println!("hart{id} has status: {status}");
            }
            StepOut
        }
        DtbObj::Property(_) => StepOver,
    });
    println!();
}

fn hart_start(id: usize, boot_hart_id: usize) {
    if id != boot_hart_id {
        println!("hart{id} is booting...");
        let ret = sbi::hart_start(
            id,
            secondary_hart_start as usize - PHYSICAL_MEMORY_OFFSET,
            0,
        );
        if ret.error != sbi::RET_SUCCESS {
            panic!("start hart{id} failed. error: {ret:?}");
        }
    } else {
        println!("hart{id} is the primary hart.");
    }
}
