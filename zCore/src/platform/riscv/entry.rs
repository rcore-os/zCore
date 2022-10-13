use super::{
    boot_page_table::BootPageTable,
    consts::{kernel_mem_info, MAX_HART_NUM, STACK_PAGES_PER_HART},
};
use core::arch::asm;
use dtb_walker::{Dtb, DtbObj, HeaderError::*, Property, Str, WalkOperation::*};
use kernel_hal::KernelConfig;

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
        "call {select_stack}", // 设置启动栈
        "j    {main}",         // 进入 rust
        select_stack = sym select_stack,
        main         = sym primary_rust_main,
        options(noreturn)
    )
}

/// 副核入口。此前副核被 bootloader/see 阻塞。
///
/// # Safety
///
/// 裸函数。
#[naked]
unsafe extern "C" fn secondary_hart_start(hartid: usize) -> ! {
    asm!(
        "call {select_stack}", // 设置启动栈
        "j    {main}",         // 进入 rust
        select_stack = sym select_stack,
        main         = sym secondary_rust_main,
        options(noreturn)
    )
}

/// 启动页表
static mut BOOT_PAGE_TABLE: BootPageTable = BootPageTable::ZERO;

/// 主核启动。
extern "C" fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    // 清零 bss 段
    extern "C" {
        static mut sbss: u64;
        static mut ebss: u64;
    }
    unsafe { r0::zero_bss(&mut sbss, &mut ebss) };
    // 使能启动页表
    let sstatus = unsafe {
        BOOT_PAGE_TABLE.init();
        BOOT_PAGE_TABLE.launch()
    };
    let mem_info = kernel_mem_info();
    // 检查设备树
    let dtb = unsafe {
        Dtb::from_raw_parts_filtered((device_tree_paddr + mem_info.offset()) as _, |e| {
            matches!(e, Misaligned(4) | LastCompVersion(_))
        })
    }
    .unwrap();
    // 打印启动信息
    println!(
        "
boot page table launched, sstatus = {sstatus:#x}
kernel (physical): {:016x}..{:016x}
kernel (remapped): {:016x}..{:016x}
device tree:       {device_tree_paddr:016x}..{:016x}
",
        mem_info.paddr_base,
        mem_info.paddr_base + mem_info.size,
        mem_info.vaddr_base,
        mem_info.vaddr_base + mem_info.size,
        device_tree_paddr + dtb.total_size(),
    );
    // 启动副核
    boot_secondary_harts(
        hartid,
        &dtb,
        secondary_hart_start as usize - mem_info.offset(),
    );
    // 转交控制权
    crate::primary_main(KernelConfig {
        phys_to_virt_offset: mem_info.offset(),
        dtb_paddr: device_tree_paddr,
        dtb_size: dtb.total_size() as _,
    });
    sbi_rt::system_reset(sbi_rt::Shutdown, sbi_rt::NoReason);
    unreachable!()
}

/// 副核启动。
extern "C" fn secondary_rust_main() -> ! {
    let _ = unsafe { BOOT_PAGE_TABLE.launch() };
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
        "   mv   tp, a0",
        "   addi t0, a0,  1
            la   sp, {stack}
            li   t1, {len_per_hart}
         1: add  sp, sp, t1
            addi t0, t0, -1
            bnez t0, 1b
            ret
        ",
        stack        =   sym BOOT_STACK,
        len_per_hart = const STACK_LEN_PER_HART,
        options(noreturn)
    )
}

// 启动副核
fn boot_secondary_harts(boot_hartid: usize, dtb: &Dtb, start_addr: usize) {
    if sbi_rt::probe_extension(sbi_rt::Hsm).is_unavailable() {
        println!("HSM SBI extension is not supported for current SEE.");
        return;
    }

    let mut cpus = false;
    let mut cpu: Option<usize> = None;
    dtb.walk(|path, obj| match obj {
        DtbObj::SubNode { name } => {
            if path.is_root() {
                if name == Str::from("cpus") {
                    // 进入 cpus 节点
                    cpus = true;
                    StepInto
                } else if cpus {
                    // 已离开 cpus 节点
                    if let Some(hartid) = cpu.take() {
                        hart_start(boot_hartid, hartid, start_addr);
                    }
                    Terminate
                } else {
                    // 其他节点
                    StepOver
                }
            } else if path.name() == Str::from("cpus") {
                // 如果没有 cpu 序号，肯定是单核的
                if name == Str::from("cpu") {
                    return Terminate;
                }
                if name.starts_with("cpu@") {
                    let id: usize = usize::from_str_radix(
                        unsafe { core::str::from_utf8_unchecked(&name.as_bytes()[4..]) },
                        16,
                    )
                    .unwrap();
                    if let Some(hartid) = cpu.replace(id) {
                        hart_start(boot_hartid, hartid, start_addr);
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
            if path.name().starts_with("cpu@") && status != Str::from("okay") =>
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

fn hart_start(boot_hartid: usize, hartid: usize, start_addr: usize) {
    if hartid != boot_hartid {
        println!("hart{hartid} is booting...");
        let ret = sbi_rt::hart_start(hartid, start_addr, 0);
        if ret.is_err() {
            panic!("start hart{hartid} failed. error: {ret:?}");
        }
    } else {
        println!("hart{hartid} is the primary hart.");
    }
}
