use alloc::boxed::Box;
use riscv::register::{sie, sstatus};
use spin::Mutex;

use super::{plic, trap};
use crate::utils::irq_manager::IrqManager;

// IRQ
const TIMER: u32 = 5;
const U_PLIC: u32 = 8;
const S_PLIC: u32 = 9;
const M_PLIC: u32 = 11;

lazy_static! {
    static ref IRQ_MANAGER: Mutex<IrqManager> = Mutex::new(IrqManager::new(1, 15));
}

#[allow(dead_code)]
fn init_soft() {
    unsafe { sie::set_ssoft() };
    info!("+++ setup soft int! +++");
}

fn init_ext() {
    unsafe { sie::set_sext() };
    plic::init();
    info!("+++ Setting up PLIC +++");
}

fn init_irq() {
    let mut im = IRQ_MANAGER.lock();
    // 模拟参照了x86_64,把timer处理函数也放进去了
    im.register_handler(TIMER, Box::new(trap::super_timer)).ok();
    // im.register_handler(Keyboard, Box::new(keyboard));
    im.register_handler(S_PLIC, Box::new(plic::handle_interrupt))
        .ok();
}

pub(super) fn init() {
    unsafe { sstatus::set_sie() };
    init_ext();
    init_irq();
    info!("+++ setup interrupt OK +++");
}

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn handle_irq(vector: u32) {
            debug!("PLIC handle: {:#x}", vector);
            IRQ_MANAGER.lock().handle(vector);
        }
    }
}
