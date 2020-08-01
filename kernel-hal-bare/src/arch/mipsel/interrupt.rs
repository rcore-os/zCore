use crate::drivers::IRQ_MANAGER;
use mips::addr::*;
use mips::paging::PageTable as MIPSPageTable;
use mips::registers::cp0;
use trapframe::TrapFrame;

/// Initialize interrupt
pub fn intr_init() {
    unsafe {
        trapframe::init();
    }
    let mut status = cp0::status::read();
    // Enable IPI
    // status.enable_soft_int0();
    // status.enable_soft_int1();
    // Enable serial interrupt
    status.enable_hard_int2();
    // Enable clock interrupt in timer::init
    // status.enable_hard_int5();

    cp0::status::write(status);
    info!("interrupt: init end");
}

#[export_name = "hal_page_fault"]
pub fn is_page_fault(trap: usize) -> bool {
    use cp0::cause::Exception as E;
    let cause = cp0::cause::Cause { bits: trap as u32 };
    match cause.cause() {
        E::TLBModification | E::TLBLoadMiss | E::TLBStoreMiss => true,
        _ => false,
    }
}

#[export_name = "hal_is_syscall"]
pub fn is_syscall(trap: usize) -> bool {
    use cp0::cause::Exception as E;
    let cause = cp0::cause::Cause { bits: trap as u32 };
    match cause.cause() {
        E::Syscall => true,
        _ => false,
    }
}

#[export_name = "hal_is_intr"]
pub fn is_intr(trap: usize) -> bool {
    use cp0::cause::Exception as E;
    let cause = cp0::cause::Cause { bits: trap as u32 };
    match cause.cause() {
        E::Interrupt => true,
        _ => false,
    }
}

#[export_name = "hal_is_timer_intr"]
pub fn is_timer_intr(trap: usize) -> bool {
    use cp0::cause::Exception as E;
    let cause = cp0::cause::Cause { bits: trap as u32 };
    match cause.cause() {
        E::Interrupt => trap & (1 << 30) != 0,
        _ => false,
    }
}

#[export_name = "hal_is_reserved_inst"]
pub fn is_reserved_inst(trap: usize) -> bool {
    use cp0::cause::Exception as E;
    let cause = cp0::cause::Cause { bits: trap as u32 };
    match cause.cause() {
        E::ReservedInstruction => true,
        _ => false,
    }
}

#[export_name = "hal_wait_for_interrupt"]
pub fn wait_for_interrupt() {
    cp0::status::enable_interrupt();
    cp0::status::disable_interrupt();
}

#[export_name = "hal_irq_enable"]
pub fn irq_enable(_irq: u32) {
    // unimplemented!()
    warn!("unimplemented irq_enable");
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    use cp0::cause::Exception as E;
    let cause = cp0::cause::Cause {
        bits: tf.cause as u32,
    };
    info!("Exception @ CPU{}: {:?} ", 0, cause.cause());
    match cause.cause() {
        E::Interrupt => interrupt_dispatcher(tf),
        // E::Syscall => syscall(tf),
        E::TLBModification => page_fault(tf),
        E::TLBLoadMiss => page_fault(tf),
        E::TLBStoreMiss => page_fault(tf),
        UNKNOWN => {
            error!("Unhandled Exception @ CPU{}: {:?} ", 0, UNKNOWN);
        }
    }
    trace!("Interrupt end");
}

fn interrupt_dispatcher(tf: &mut TrapFrame) {
    let cause = cp0::cause::Cause {
        bits: tf.cause as u32,
    };
    let pint = cause.pending_interrupt();
    trace!("  Interrupt {:08b} ", pint);
    if (pint & 0b100_000_00) != 0 {
        timer();
    } else if (pint & 0b011_111_00) != 0 {
        for i in 0..6 {
            if (pint & (1 << i)) != 0 {
                IRQ_MANAGER.read().try_handle_interrupt(Some(i));
            }
        }
    } else {
        ipi();
    }
}

fn ipi() {
    debug!("IPI");
    cp0::cause::reset_soft_int0();
    cp0::cause::reset_soft_int1();
}

pub fn timer() {
    super::timer::set_next();
    crate::timer_tick();
}

fn page_fault(tf: &mut TrapFrame) {
    // TODO: set access/dirty bit
    let addr = tf.vaddr;
    // info!("\nEXCEPTION: Page Fault @ {:#x}", addr);

    let virt_addr = VirtAddr::new(addr);
    error!("{:x}", super::memory::get_page_table());
    let root_table = unsafe { &mut *(super::memory::get_page_table() as *mut MIPSPageTable) };
    let tlb_result = root_table.lookup(addr);
    match tlb_result {
        Ok(tlb_entry) => {
            trace!(
                "PhysAddr = {:x}/{:x}",
                tlb_entry.entry_lo0.get_pfn() << 12,
                tlb_entry.entry_lo1.get_pfn() << 12
            );

            let tlb_valid = if virt_addr.page_number() & 1 == 0 {
                tlb_entry.entry_lo0.valid()
            } else {
                tlb_entry.entry_lo1.valid()
            };

            if !tlb_valid {
                panic!("hhh");
                // if !crate::memory::handle_page_fault(addr) {
                //     extern "C" {
                //         fn _copy_user_start();
                //         fn _copy_user_end();
                //     }
                //     if tf.epc >= _copy_user_start as usize && tf.epc < _copy_user_end as usize {
                //         debug!("fixup for addr {:x?}", addr);
                //         tf.epc = crate::read_user_fixup as usize;
                //         return;
                //     }
                // }
            }

            tlb_entry.write_random()
        }
        Err(()) => {
            // if !crate::memory::handle_page_fault(addr) {
            //     extern "C" {
            //         fn _copy_user_start();
            //         fn _copy_user_end();
            //     }
            //     if tf.epc >= _copy_user_start as usize && tf.epc < _copy_user_end as usize {
            //         debug!("fixup for addr {:x?}", addr);
            //         tf.epc = crate::read_user_fixup as usize;
            //         return;
            //     }
            // }
            panic!("...");
        }
    }
}
