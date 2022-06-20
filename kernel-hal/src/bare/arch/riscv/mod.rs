mod drivers;
mod trap;

pub mod config;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod sbi;
pub mod timer;
pub mod vm;

use crate::{mem::phys_to_virt, utils::init_once::InitOnce, PhysAddr};
use alloc::{string::String, vec::Vec};
use core::ops::Range;

static CMDLINE: InitOnce<String> = InitOnce::new_with_default(String::new());
static INITRD_REGION: InitOnce<Option<Range<PhysAddr>>> = InitOnce::new_with_default(None);
static MEMORY_REGIONS: InitOnce<Vec<Range<PhysAddr>>> = InitOnce::new_with_default(Vec::new());

pub const fn timer_interrupt_vector() -> usize {
    trap::SUPERVISOR_TIMER_INT_VEC
}

pub fn cmdline() -> String {
    CMDLINE.clone()
}

pub fn init_ram_disk() -> Option<&'static mut [u8]> {
    INITRD_REGION.as_ref().map(|range| unsafe {
        core::slice::from_raw_parts_mut(phys_to_virt(range.start) as *mut u8, range.len())
    })
}

pub fn primary_init_early() {
    use dtb_walker::{Dtb, DtbObj, Property, WalkOperation::*};
    let mut initrd_start: Option<usize> = None;
    let mut initrd_end: Option<usize> = None;
    // smp 启动时已经检查过了，不要重复检查
    unsafe { Dtb::from_raw_parts_unchecked(phys_to_virt(crate::KCONFIG.dtb_paddr) as _) }.walk(
        |path, obj| match obj {
            DtbObj::SubNode { name } => {
                if path.last().is_empty()
                    && (matches!(name, b"chosen" | b"cpus") || name.starts_with(b"memory"))
                {
                    StepInto
                } else {
                    StepOver
                }
            }
            DtbObj::Property(Property::General { name, value }) if path.last() == b"chosen" => {
                match name.as_bytes() {
                    b"bootargs" if let Some(b'\0') = value.last() => {
                        let cmdline = String::from_utf8_lossy(&value[..value.len()-1]).into_owned();
                        info!("Load kernel cmdline from DTB: {cmdline:?}");
                        CMDLINE.init_once_by(cmdline);
                    }
                    b"linux,initrd-start" if let [a, b, c, d] = *value => {
                        initrd_start = Some(u32::from_be_bytes([a, b, c, d]) as _);
                    }
                    b"linux,initrd-end" if let [a, b, c, d] = *value => {
                        initrd_end = Some(u32::from_be_bytes([a, b, c, d]) as _);
                    }
                    _ => {}
                }
                StepOver
            }
            DtbObj::Property(Property::General { name, value }) if path.last() == b"cpus" => {
                if let b"timebase-frequency" = name.as_bytes() && let [a,b,c,d] = *value  {
                    let time_freq = u32::from_be_bytes([a, b, c, d]);
                    info!("Load CPU clock frequency from DTB: {time_freq} Hz");
                    super::cpu::CPU_FREQ_MHZ.init_once_by((time_freq / 1_000_000) as u16);
                }
                StepOver
            }
            DtbObj::Property(Property::Reg(reg)) if path.last().starts_with(b"memory") => {
                let regions = reg.collect();
                info!("Load memory regions from DTB: {regions:#x?}");
                MEMORY_REGIONS.init_once_by(regions);
                StepOver
            }
            DtbObj::Property(_) => StepOver,
        },
    );
    if let Some(s) = initrd_start && let Some(e) = initrd_end {
        let initrd_region = s..e;
        info!("Load initrd regions from DTB: {initrd_region:#x?}");
        INITRD_REGION.init_once_by(Some(initrd_region));
    }
}

pub fn primary_init() {
    vm::init();
    drivers::init().unwrap();
    // We should set first time interrupt before run into first user program
    // timer::init();
}

pub fn timer_init() {
    timer::init();
}

pub fn secondary_init() {
    vm::init();
    drivers::intc_init().unwrap();
    let plic = crate::drivers::all_irq()
        .find("riscv-plic")
        .expect("IRQ device 'riscv-plic' not initialized!");
    plic.init_hart();
}
