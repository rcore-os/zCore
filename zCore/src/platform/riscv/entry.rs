use super::{
    boot_page_table::BootPageTable,
    consts::{MAX_HART_NUM, PHYSICAL_MEMORY_OFFSET, STACK_PAGES_PER_HART},
};
use core::arch::asm;
use device_tree::parse_smp;
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

mod device_tree {
    use super::PHYSICAL_MEMORY_OFFSET;
    use serde::Deserialize;
    use serde_device_tree::{
        buildin::{NodeSeq, Reg, StrSeq},
        from_raw_mut, Dtb, DtbPtr,
    };

    #[derive(Deserialize)]
    pub(super) struct Tree<'a> {
        compatible: StrSeq<'a>,
        model: StrSeq<'a>,
        chosen: Option<Chosen<'a>>,
        cpus: Cpus<'a>,
        memory: NodeSeq<'a>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub(super) struct Chosen<'a> {
        stdout_path: Option<StrSeq<'a>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub(super) struct Cpus<'a> {
        timebase_frequency: u32,
        cpu: NodeSeq<'a>,
    }

    #[allow(dead_code)]
    #[derive(Deserialize, Debug)]
    pub(super) struct Cpu<'a> {
        compatible: StrSeq<'a>,
        device_type: StrSeq<'a>,
        status: StrSeq<'a>,
        #[serde(rename = "riscv,isa")]
        isa: StrSeq<'a>,
        #[serde(rename = "mmu-type")]
        mmu: StrSeq<'a>,
    }

    #[derive(Deserialize)]
    pub(super) struct Memory<'a> {
        device_type: StrSeq<'a>,
        reg: Reg<'a>,
    }

    pub(super) fn parse_smp(device_tree_paddr: usize) -> usize {
        let ptr = DtbPtr::from_raw((device_tree_paddr + PHYSICAL_MEMORY_OFFSET) as _).unwrap();
        let dtb = Dtb::from(ptr).share();
        let t: Tree = from_raw_mut(&dtb).unwrap();

        println!("model = {:?}", t.model);
        println!("compatible = {:?}", t.compatible);
        if let Some(chosen) = t.chosen {
            if let Some(stdout_path) = chosen.stdout_path {
                println!("stdout = {:?}", stdout_path);
            } else {
                println!("stdout not chosen");
            }
        }
        println!("cpu timebase frequency = {}", t.cpus.timebase_frequency);

        println!("number of cpu = {}", t.cpus.cpu.len());
        for cpu in t.cpus.cpu.iter() {
            println!("cpu@{}: {:?}", cpu.at(), cpu.deserialize::<Cpu>());
        }

        for item in t.memory.iter() {
            let mem: Memory = item.deserialize();
            println!(
                "memory@{}({:?}): {:#x?}",
                item.at(),
                mem.device_type,
                mem.reg
            );
        }

        t.cpus.cpu.len()
    }
}
