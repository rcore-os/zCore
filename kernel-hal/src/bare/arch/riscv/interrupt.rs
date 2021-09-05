use alloc::{boxed::Box, vec::Vec};
use riscv::register::{sie, sstatus};
use spin::Mutex;

use super::{consts, plic, serial, trap, uart};

// IRQ
const TIMER: u8 = 5;
const U_PLIC: u8 = 8;
const S_PLIC: u8 = 9;
const M_PLIC: u8 = 11;

const TABLE_SIZE: usize = 256;

type InterruptHandler = Box<dyn Fn() + Send + Sync>;

lazy_static::lazy_static! {
    static ref IRQ_TABLE: Mutex<Vec<Option<InterruptHandler>>> = Default::default();
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

/// Add a handle to IRQ table. Return the specified irq or an allocated irq on success
fn irq_add_handle(irq: u8, handler: InterruptHandler) -> Option<u8> {
    info!("IRQ add handle {:#x?}", irq);
    let mut table = IRQ_TABLE.lock();
    // allocate a valid irq number
    // why?
    if irq == 0 {
        let mut id = 0x20;
        while id < table.len() {
            if table[id].is_none() {
                table[id] = Some(handler);
                return Some(id as u8);
            }
            id += 1;
        }
        return None;
    }

    match table[irq as usize] {
        Some(_) => None,
        None => {
            table[irq as usize] = Some(handler);
            Some(irq)
        }
    }
}

fn init_irq_table() {
    let mut table = IRQ_TABLE.lock();
    for _ in 0..TABLE_SIZE {
        table.push(None);
    }
}

fn init_irq() {
    init_irq_table();
    irq_add_handle(TIMER, Box::new(trap::super_timer)); //模拟参照了x86_64,把timer处理函数也放进去了
                                                        //irq_add_handle(Keyboard, Box::new(keyboard));
    irq_add_handle(S_PLIC, Box::new(plic::handle_interrupt));
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
        fn handle_irq(irq: u32) {
            debug!("PLIC handle: {:#x}", irq);
            let table = IRQ_TABLE.lock();
            match &table[irq as usize] {
                Some(f) => f(),
                None => panic!("unhandled U-mode external IRQ number: {}", irq),
            }
        }
    }
}
