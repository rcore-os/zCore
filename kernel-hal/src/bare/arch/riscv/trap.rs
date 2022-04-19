use riscv::register::scause;
use trapframe::TrapFrame;

use crate::context::TrapReason;
pub(super) const SUPERVISOR_TIMER_INT_VEC: usize = 5; // scause::Interrupt::SupervisorTimer

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

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let scause = scause::read();
    trace!("trap happened: {:?}", TrapReason::from(scause));
    match TrapReason::from(scause) {
        TrapReason::SoftwareBreakpoint => breakpoint(&mut tf.sepc),
        TrapReason::PageFault(vaddr, flags) => crate::KHANDLER.handle_page_fault(vaddr, flags),
        TrapReason::Interrupt(vector) => {
            crate::interrupt::handle_irq(vector);
            if vector == SUPERVISOR_TIMER_INT_VEC {
                executor::handle_timeout();
            }
        }
        other => panic!("Undefined trap: {:x?} {:#x?}", other, tf),
    }
}
