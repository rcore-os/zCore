use riscv::register::scause::{self, Exception, Trap};
use riscv::register::stval;
use trapframe::TrapFrame;

use crate::MMUFlags;

fn breakpoint(sepc: &mut usize) {
    info!("Exception::Breakpoint: A breakpoint set @0x{:x} ", sepc);

    //sepc为触发中断指令ebreak的地址
    //防止无限循环中断，让sret返回时跳转到sepc的下一条指令地址
    *sepc += 2
}

pub(super) fn super_timer() {
    super::timer::timer_set_next();
    crate::timer::timer_tick();

    //发生外界中断时，epc的指令还没有执行，故无需修改epc到下一条
}

pub(super) fn super_soft() {
    super::sbi::clear_ipi();
    info!("Interrupt::SupervisorSoft!");
}

fn page_fault(access_flags: MMUFlags) {
    crate::KHANDLER.handle_page_fault(stval::read(), access_flags);
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let sepc = tf.sepc;
    let scause = scause::read();
    match scause.cause() {
        Trap::Exception(Exception::Breakpoint) => breakpoint(&mut tf.sepc),
        Trap::Exception(Exception::IllegalInstruction) => {
            panic!("IllegalInstruction: {:#x}, sepc={:#x}", stval::read(), sepc)
        }
        Trap::Exception(Exception::LoadFault) => {
            panic!("Load access fault: {:#x}, sepc={:#x}", stval::read(), sepc)
        }
        Trap::Exception(Exception::StoreFault) => {
            panic!("Store access fault: {:#x}, sepc={:#x}", stval::read(), sepc)
        }
        Trap::Exception(Exception::LoadPageFault) => page_fault(MMUFlags::READ),
        Trap::Exception(Exception::StorePageFault) => page_fault(MMUFlags::WRITE),
        Trap::Exception(Exception::InstructionPageFault) => page_fault(MMUFlags::EXECUTE),
        Trap::Interrupt(_) => crate::interrupt::handle_irq(scause.code()),
        _ => panic!("Undefined Trap: {:?}", scause.cause()),
    }
}
