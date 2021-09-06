use alloc::boxed::Box;
use riscv::register::{sie, sstatus};
use spin::Mutex;

use super::{consts, plic, serial, trap, uart};
use crate::interrupt::IrqManager;

pub(crate) const IRQ_MIN_ID: u32 = 0x1;
pub(crate) const IRQ_MAX_ID: u32 = 0xff;

// IRQ
const TIMER: u32 = 5;
const U_PLIC: u32 = 8;
const S_PLIC: u32 = 9;
const M_PLIC: u32 = 11;

lazy_static::lazy_static! {
    static ref IRQ_MANAGER: Mutex<IrqManager> = Mutex::default();
}

#[allow(dead_code)]
fn init_soft() {
    unsafe { sie::set_ssoft() };
    sbi_println!("+++ setup soft int! +++");
}

fn init_ext() {
    unsafe { sie::set_sext() };
    plic::init();
    sbi_println!("+++ Setting up PLIC +++");
}

fn init_uart() {
    uart::init(consts::UART_BASE);

    //但当没有SBI_CONSOLE_PUTCHAR时，却为什么不行？
    serial::uart_print_fmt(format_args!("UART output testing\n\r"));

    sbi_println!("+++ Setting up UART interrupts +++");
}

fn init_irq() {
    let mut im = IRQ_MANAGER.lock();
    // 模拟参照了x86_64,把timer处理函数也放进去了
    im.add_handler(TIMER, Box::new(trap::super_timer)).ok();
    // im.add_handler(Keyboard, Box::new(keyboard));
    im.add_handler(S_PLIC, Box::new(plic::handle_interrupt))
        .ok();
}

pub(super) fn init() {
    unsafe { sstatus::set_sie() };
    init_uart();
    init_ext();
    init_irq();
    sbi_println!("+++ setup interrupt +++");
}

hal_fn_impl! {
    impl mod crate::defs::interrupt {
        fn handle_irq(vector: u32) {
            debug!("PLIC handle: {:#x}", vector);
            IRQ_MANAGER.lock().handle(vector);
        }
    }
}
