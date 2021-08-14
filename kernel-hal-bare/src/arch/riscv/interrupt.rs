use alloc::boxed::Box;
use alloc::vec::Vec;
use riscv::register::{
    satp,
    scause::{self, Exception, Interrupt, Trap},
    sie, sstatus, stval,
};
use spin::Mutex;
use trapframe::{TrapFrame, UserContext};

use super::plic;
use super::sbi;
use super::consts::PHYSICAL_MEMORY_OFFSET;
use super::timer_set_next;
use super::{map_range, phys_to_virt, putfmt};

pub fn init() {
    unsafe {
        sstatus::set_sie();
        sie::set_sext();
        init_ext();
    }
    info!("+++ setup interrupt +++");
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
        Trap::Interrupt(Interrupt::SupervisorExternal) => {
            if let Some(id) = plic::next() {
                match id {
                    1..=8 => {
                        //virtio::handle_interrupt(interrupt);
                        info!("plic virtio external interrupt: {}", id);
                    }
                    10 => serial(),
                    _ => info!("Unknown external interrupt: {}", id),
                }
                plic::complete(id);
            }
        }
        //Trap::Interrupt(Interrupt::SupervisorExternal) => irq_handle(code as u8),
        _ => panic!("Undefined Trap: {:#x} {:#x}", is_int, code),
    }
}

fn breakpoint(sepc: &mut usize) {
    info!("Exception::Breakpoint: A breakpoint set @0x{:x} ", sepc);

    //sepc为触发中断指令ebreak的地址
    //防止无限循环中断，让sret返回时跳转到sepc的下一条指令地址
    *sepc += 2;
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

    use super::PageTableImpl;
    use kernel_hal::{MMUFlags, PageTableTrait};
    use riscv::addr::{Page, PhysAddr, VirtAddr};
    use riscv::paging::{PageTableFlags as PTF, Rv39PageTable, *};

    //let mut flags = PTF::VALID;
    let code = this_scause.code();
    let mut flags = if code == 15 {
        //MMUFlags::WRITE ???
        MMUFlags::READ | MMUFlags::WRITE
    } else if code == 12 {
        MMUFlags::EXECUTE
    } else {
        MMUFlags::READ
    };

    let linear_offset = if stval >= PHYSICAL_MEMORY_OFFSET {
        // Kernel
        PHYSICAL_MEMORY_OFFSET
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

    let mut pti = PageTableImpl {
        root_paddr: satp::read().frame().start_address().as_usize(),
    };

    let page = Page::of_addr(VirtAddr::new(vaddr));
    if let Ok(pte) = pti.get().ref_entry(page) {
        let pte = unsafe { &mut *(pte as *mut PageTableEntry) };
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

fn super_timer() {
    timer_set_next();
    super::timer_tick();

    //发生外界中断时，epc的指令还没有执行，故无需修改epc到下一条
}

fn serial() {
    let c = super::UART.lock().receive();
    super::serial_put(c);
}

pub fn init_ext() {
    // Qemu virt
    // UART0 = 10
    plic::set_priority(10, 7);
    plic::set_threshold(0);
    plic::enable(10);

    info!("+++ Setting up PLIC +++");
}

fn super_soft() {
    sbi::clear_ipi();
    info!("Interrupt::SupervisorSoft!");
}

pub fn init_soft() {
    unsafe {
        sie::set_ssoft();
    }
    info!("+++ setup soft int! +++");
}

#[export_name = "fetch_trap_num"]
pub fn fetch_trap_num(_context: &UserContext) -> usize {
    scause::read().bits()
}

pub fn wait_for_interrupt() {
    unsafe {
        // enable interrupt and disable
        let sie = riscv::register::sstatus::read().sie();
        riscv::register::sstatus::set_sie();
        riscv::asm::wfi();
        if !sie {
            riscv::register::sstatus::clear_sie();
        }
    }
}

fn timer() {
    super::timer_tick();
}
