use riscv::register::scause::{self, Exception, Interrupt, Trap};
use riscv::register::stval;
use trapframe::TrapFrame;

use super::{plic, sbi};
use crate::MMUFlags;

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
            panic!("IllegalInstruction: {:#x}->{:#x}", sepc, stval::read())
        }
        Trap::Exception(Exception::LoadFault) => {
            panic!("Load access fault: {:#x}->{:#x}", sepc, stval::read())
        }
        Trap::Exception(Exception::StoreFault) => {
            panic!("Store access fault: {:#x}->{:#x}", sepc, stval::read())
        }
        Trap::Exception(Exception::LoadPageFault) => page_fault(MMUFlags::READ),
        Trap::Exception(Exception::StorePageFault) => page_fault(MMUFlags::WRITE),
        Trap::Exception(Exception::InstructionPageFault) => page_fault(MMUFlags::EXECUTE),
        Trap::Interrupt(Interrupt::SupervisorTimer) => super_timer(),
        Trap::Interrupt(Interrupt::SupervisorSoft) => super_soft(),
        Trap::Interrupt(Interrupt::SupervisorExternal) => plic::handle_interrupt(),
        //Trap::Interrupt(Interrupt::SupervisorExternal) => irq_handle(code as u8),
        _ => panic!("Undefined Trap: {:?}", scause.cause()),
    }
}
