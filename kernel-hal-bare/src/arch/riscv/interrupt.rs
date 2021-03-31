use riscv::register::{
	scause::{
		self,
        Scause,
		Trap,
		Exception,
		Interrupt,
	},
    sie,
	sepc,
	stval,
	stvec,
	sscratch,
    sstatus,
	sstatus::Sstatus,
};
use trapframe::TrapFrame;

/*
use crate::timer::{
	TICKS,
	clock_set_next_event,
    clock_close,
};
*/

//use crate::context::TrapFrame;
use super::sbi;
use super::plic;
use super::uart;

use crate::{putfmt, phys_to_virt};
use super::clock_set_next_event;

pub fn init(){
	unsafe{
		extern "C" {
			fn trap_entry();
		}

		sscratch::write(0);
		stvec::write(trap_entry as usize, stvec::TrapMode::Direct);

		sstatus::set_sie();

        //init_m();

        sie::set_sext();
        init_ext();

        init_uart();
	}
	bare_println!("+++ setup interrupte! +++");
}

#[no_mangle]
pub fn trap_handler(tf: &mut TrapFrame){
    let sepc = tf.sepc;
    let scause = scause::read();
    let stval = stval::read();
    let is_int = scause.bits() >> 63;
    let code = scause.bits() & !(1 << 63);
	match scause.cause() {
		Trap::Exception(Exception::Breakpoint) => breakpoint(&mut tf.sepc),
		Trap::Exception(Exception::IllegalInstruction) => panic!("IllegalInstruction: {:#x}->{:#x}", sepc, stval),
        Trap::Exception(Exception::LoadFault) => panic!("Load access fault: {:#x}->{:#x}", sepc, stval),
        Trap::Exception(Exception::StoreFault) => panic!("Store access fault: {:#x}->{:#x}", sepc, stval),
        Trap::Exception(Exception::LoadPageFault) => page_fault(stval, tf),
        Trap::Exception(Exception::StorePageFault) => page_fault(stval, tf),
        Trap::Exception(Exception::InstructionPageFault) => page_fault(stval, tf),
		Trap::Interrupt(Interrupt::SupervisorTimer) => super_timer(),
		Trap::Interrupt(Interrupt::SupervisorSoft) => super_soft(),
		Trap::Interrupt(Interrupt::SupervisorExternal) => external(),
		_ => panic!("Undefined Trap: {:#x} {:#x}", is_int, code)
	}
}

fn external() {
	// assume only keyboard interrupt
	let mut c = sbi::console_getchar();
    if c <= 255 {
        if c == '\r' as usize {
            c = '\n' as usize;
        }
        super::serial_put(c as u8);
    }
}

fn breakpoint(sepc: &mut usize){
	bare_println!("A breakpoint set @0x{:x} ", sepc);

	//sepc为触发中断指令ebreak的地址
	//防止无限循环中断，让sret返回时跳转到sepc的下一条指令地址
	*sepc +=2
}

fn page_fault(stval: usize, tf: &mut TrapFrame){
    panic!("EXCEPTION: Page Fault @ {:#x}->{:#x}", tf.sepc, stval);
}

fn super_timer(){
    clock_set_next_event();

    //bare_println!("Tick");
    // bare_print!(".");

	//发生外界中断时，epc的指令还没有执行，故无需修改epc到下一条
}

fn init_uart(){
    uart::Uart::new(phys_to_virt(0x1000_0000)).simple_init();

    bare_println!("+++ Setting up UART interrupts +++");
}

pub fn init_ext(){
    // Qemu virt
    // UART0 = 10
    plic::set_priority(10, 7);
    plic::set_threshold(0);
    plic::enable(10);

    bare_println!("+++ Setting up PLIC +++");
}

fn super_soft(){
    sbi::clear_ipi();
    bare_println!("Interrupt::SupervisorSoft!");
}

pub fn init_soft(){
    unsafe {
        sie::set_ssoft();
    }
	bare_println!("+++ setup soft int! +++");
}
