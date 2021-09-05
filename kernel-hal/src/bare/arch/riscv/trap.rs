use riscv::register::scause::{self, Exception, Interrupt, Trap};
use riscv::register::{satp, stval};
use trapframe::TrapFrame;

use super::{consts, plic, sbi};

fn breakpoint(sepc: &mut usize) {
    sbi_println!("Exception::Breakpoint: A breakpoint set @0x{:x} ", sepc);

    //sepc为触发中断指令ebreak的地址
    //防止无限循环中断，让sret返回时跳转到sepc的下一条指令地址
    *sepc += 2
}

pub(super) fn super_timer() {
    super::timer::timer_set_next();
    crate::timer::timer_tick();

    //sbi_print!(".");

    //发生外界中断时，epc的指令还没有执行，故无需修改epc到下一条
}

fn super_soft() {
    sbi::clear_ipi();
    sbi_println!("Interrupt::SupervisorSoft!");
}

fn page_fault(stval: usize, tf: &mut TrapFrame) {
    let this_scause = scause::read();
    info!(
        "EXCEPTION Page Fault: {:?} @ {:#x}->{:#x}",
        this_scause.cause(),
        tf.sepc,
        stval
    );
    let vaddr = stval;

    use crate::vm::{PageTable, PageTableTrait};
    use crate::MMUFlags;
    use riscv::addr::{Page, VirtAddr};
    use riscv::paging::{Mapper, PageTableEntry as PTE};

    //let mut flags = PTF::VALID;
    let code = this_scause.code();
    let flags = if code == 15 {
        //MMUFlags::WRITE ???
        MMUFlags::READ | MMUFlags::WRITE
    } else if code == 12 {
        MMUFlags::EXECUTE
    } else {
        MMUFlags::READ
    };

    let linear_offset = if stval >= consts::PHYSICAL_MEMORY_OFFSET {
        // Kernel
        consts::PHYSICAL_MEMORY_OFFSET
    } else {
        // User
        0
    };

    /*
    let current =
        unsafe { &mut *(phys_to_virt(satp::read().frame().start_address().as_usize()) as *mut PageTable) };
    let mut pt = Rv39PageTable::new(current, PHYSICAL_MEMORY_OFFSET);
    map_range(&mut pt, vaddr, vaddr, linear_offset, flags);
    */

    let mut pti = PageTable {
        root_paddr: satp::read().frame().start_address().as_usize(),
    };

    let page = Page::of_addr(VirtAddr::new(vaddr));
    if let Ok(pte) = pti.get().ref_entry(page) {
        let pte = unsafe { &mut *(pte as *mut PTE) };
        if !pte.is_unused() {
            debug!(
                "PageAlreadyMapped -> {:#x?}, {:?}",
                pte.addr().as_usize(),
                pte.flags()
            );
            //TODO update flags

            pti.unmap(vaddr).unwrap();
        }
    };
    pti.map(vaddr, vaddr - linear_offset, flags).unwrap();
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let sepc = tf.sepc;
    let scause = scause::read();
    let stval = stval::read();
    let is_int = scause.bits() >> 63;
    let code = scause.bits() & !(1 << 63);

    match scause.cause() {
        Trap::Exception(Exception::Breakpoint) => breakpoint(&mut tf.sepc),
        Trap::Exception(Exception::IllegalInstruction) => {
            panic!("IllegalInstruction: {:#x}->{:#x}", sepc, stval)
        }
        Trap::Exception(Exception::LoadFault) => {
            panic!("Load access fault: {:#x}->{:#x}", sepc, stval)
        }
        Trap::Exception(Exception::StoreFault) => {
            panic!("Store access fault: {:#x}->{:#x}", sepc, stval)
        }
        Trap::Exception(Exception::LoadPageFault) => page_fault(stval, tf),
        Trap::Exception(Exception::StorePageFault) => page_fault(stval, tf),
        Trap::Exception(Exception::InstructionPageFault) => page_fault(stval, tf),
        Trap::Interrupt(Interrupt::SupervisorTimer) => super_timer(),
        Trap::Interrupt(Interrupt::SupervisorSoft) => super_soft(),
        Trap::Interrupt(Interrupt::SupervisorExternal) => plic::handle_interrupt(),
        //Trap::Interrupt(Interrupt::SupervisorExternal) => irq_handle(code as u8),
        _ => panic!("Undefined Trap: {:#x} {:#x}", is_int, code),
    }
}
