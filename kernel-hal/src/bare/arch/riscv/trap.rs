use crate::thread::{get_current_thread, set_current_thread};
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
    #[allow(deprecated)]
    sbi_rt::legacy::clear_ipi();
    info!("Interrupt::SupervisorSoft!");
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let scause = scause::read();
    trace!("kernel trap happened: {:?}", TrapReason::from(scause));
    trace!(
        "sepc = 0x{:x} pgtoken = 0x{:x}",
        tf.sepc,
        crate::vm::current_vmtoken()
    );
    match TrapReason::from(scause) {
        TrapReason::SoftwareBreakpoint => breakpoint(&mut tf.sepc),
        TrapReason::PageFault(vaddr, flags) => crate::KHANDLER.handle_page_fault(vaddr, flags),
        TrapReason::Interrupt(vector) => {
            crate::interrupt::handle_irq(vector);
            if vector == SUPERVISOR_TIMER_INT_VEC {
                let current_thread = get_current_thread();
                set_current_thread(None);
                executor::handle_timeout();
                set_current_thread(current_thread);
            }
        }
        other => panic!("Undefined trap: {:x?} {:#x?}", other, tf),
    }
}
