use alloc::boxed::Box;
use alloc::vec::Vec;
use riscv::register::{
    satp,
    scause::{self, Exception, Interrupt, Trap},
    sie, sstatus, stval,
};
use spin::Mutex;
use trapframe::{TrapFrame, UserContext};

use super::{plic, uart, sbi, timer_set_next};
use super::consts::{PHYSICAL_MEMORY_OFFSET, UART_BASE, UART0_INT_NUM};
use crate::{map_range, phys_to_virt, putfmt};

const TABLE_SIZE: usize = 256;
pub type InterruptHandle = Box<dyn Fn() + Send + Sync>;
lazy_static! {
    static ref IRQ_TABLE: Mutex<Vec<Option<InterruptHandle>>> = Default::default();
}

fn init_irq() {
    init_irq_table();
    irq_add_handle(Timer, Box::new(super_timer)); //模拟参照了x86_64,把timer处理函数也放进去了
                                                  //irq_add_handle(Keyboard, Box::new(keyboard));
    irq_add_handle(S_PLIC, Box::new(plic::handle_interrupt));
}

pub fn init() {
    unsafe {
        sstatus::set_sie();

        init_uart();

        sie::set_sext();
        init_ext();
    }

    init_irq();

    bare_println!("+++ setup interrupt +++");
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

fn init_irq_table() {
    let mut table = IRQ_TABLE.lock();
    for _ in 0..TABLE_SIZE {
        table.push(None);
    }
}

#[export_name = "hal_irq_handle"]
pub fn irq_handle(irq: u8) {
    debug!("PLIC handle: {:#x}", irq);
    let table = IRQ_TABLE.lock();
    match &table[irq as usize] {
        Some(f) => f(),
        None => panic!("unhandled U-mode external IRQ number: {}", irq),
    }
}

/// Add a handle to IRQ table. Return the specified irq or an allocated irq on success
fn irq_add_handle(irq: u8, handle: InterruptHandle) -> Option<u8> {
    info!("IRQ add handle {:#x?}", irq);
    let mut table = IRQ_TABLE.lock();
    // allocate a valid irq number
    // why?
    if irq == 0 {
        let mut id = 0x20;
        while id < table.len() {
            if table[id].is_none() {
                table[id] = Some(handle);
                return Some(id as u8);
            }
            id += 1;
        }
        return None;
    }

    match table[irq as usize] {
        Some(_) => None,
        None => {
            table[irq as usize] = Some(handle);
            Some(irq)
        }
    }
}

fn irq_remove_handle(irq: u8) -> bool {
    info!("IRQ remove handle {:#x?}", irq);
    let irq = irq as usize;
    let mut table = IRQ_TABLE.lock();
    match table[irq] {
        Some(_) => {
            table[irq] = None;
            false
        }
        None => true,
    }
}

fn breakpoint(sepc: &mut usize) {
    bare_println!("Exception::Breakpoint: A breakpoint set @0x{:x} ", sepc);

    //sepc为触发中断指令ebreak的地址
    //防止无限循环中断，让sret返回时跳转到sepc的下一条指令地址
    *sepc += 2
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

    use crate::PageTableImpl;
    use kernel_hal::{MMUFlags, paging::PageTableTrait};
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

    //bare_print!(".");

    //发生外界中断时，epc的指令还没有执行，故无需修改epc到下一条
}

fn init_uart() {
    uart::Uart::new(phys_to_virt(UART_BASE)).simple_init();

    //但当没有SBI_CONSOLE_PUTCHAR时，却为什么不行？
    super::putfmt_uart(format_args!("{}", "UART output testing\n\r"));

    bare_println!("+++ Setting up UART interrupts +++");
}

//被plic串口中断调用
pub fn try_process_serial() -> bool {
    match super::getchar_option() {
        Some(ch) => {
            super::serial_put(ch);
            true
        }
        None => false,
    }
}

pub fn init_ext() {
    // Qemu virt UART0 = 10
    // ALLWINNER D1 UART0 = 18
    plic::set_priority(UART0_INT_NUM, 7);
    plic::set_threshold(0);
    plic::enable(UART0_INT_NUM);

    bare_println!("+++ Setting up PLIC +++");
}

fn super_soft() {
    sbi::clear_ipi();
    bare_println!("Interrupt::SupervisorSoft!");
}

pub fn init_soft() {
    unsafe {
        sie::set_ssoft();
    }
    bare_println!("+++ setup soft int! +++");
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

/*
 * 改道uart::handle_interrupt()中
 *
fn com1() {
    let c = super::COM1.lock().receive();
    super::serial_put(c);
}
*/

/*
fn keyboard() {
    use pc_keyboard::{DecodedKey, KeyCode};
    if let Some(key) = super::keyboard::receive() {
        match key {
            DecodedKey::Unicode(c) => super::serial_put(c as u8),
            DecodedKey::RawKey(code) => {
                let s = match code {
                    KeyCode::ArrowUp => "\u{1b}[A",
                    KeyCode::ArrowDown => "\u{1b}[B",
                    KeyCode::ArrowRight => "\u{1b}[C",
                    KeyCode::ArrowLeft => "\u{1b}[D",
                    _ => "",
                };
                for c in s.bytes() {
                    super::serial_put(c);
                }
            }
        }
    }
}
*/

// IRQ
const Timer: u8 = 5;
const U_PLIC: u8 = 8;
const S_PLIC: u8 = 9;
const M_PLIC: u8 = 11;

//const Keyboard: u8 = 1;
//const COM2: u8 = 3;
const COM1: u8 = 0;
//const IDE: u8 = 14;
