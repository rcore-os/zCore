use super::uart;
use crate::putfmt; //For bare_println

const MMODE: usize = 0;

// k210
//const MMODE: usize = 1;

//通过MMIO地址对平台级中断控制器PLIC的寄存器进行设置
//
//Source 1 priority: 0x0c000004
//Source 2 priority: 0x0c000008
const PLIC_PRIORITY:   usize = 0x0c00_0000;
//Pending 32位寄存器，每一位标记一个中断源ID
const PLIC_PENDING:    usize = 0x0c00_1000;

//Target 0 threshold: 0x0c200000
//Target 0 claim    : 0x0c200004
//
//Target 1 threshold: 0x0c201000 *
//Target 1 claim    : 0x0c201004 *

const PLIC_THRESHOLD:  usize = if MMODE == 1 { 0x0c200000 }else{ 0x0c201000 };
const PLIC_CLAIM:      usize = if MMODE == 1 { 0x0c200004 }else{ 0x0c201004 };

//注意一个核的不同权限模式是不同Target
//Target: 0  1  2        3  4  5
// Hart0: M  S  U Hart1: M  S  U
//
//target 0 enable: 0x0c002000
//target 1 enable: 0x0c002080 *
const PLIC_INT_ENABLE: usize = if MMODE == 1 { 0x0c002000 }else{ 0x0c002080 }; //基于opensbi后一般运行于Hart0 S态，故为Target1

//PLIC是async cause 11
//声明claim会清除中断源上的相应pending位。
//即使mip寄存器的MEIP位没有置位, 也可以claim; 声明不被阀值寄存器的设置影响；
//获取按优先级排序后的下一个可用的中断ID
pub fn next() -> Option<u32> {
	let claim_reg = PLIC_CLAIM as *const u32;
	let claim_no;
	unsafe {
		claim_no = claim_reg.read_volatile();
	}
	if claim_no == 0 {
		None //没有可用中断待定
	}else{
		Some(claim_no)
	}
}

//claim时，PLIC不再从该相同设备监听中断
//写claim寄存器，告诉PLIC处理完成该中断
// id 应该来源于next()函数
pub fn complete(id: u32) {
	let complete_reg = PLIC_CLAIM as *mut u32; //和claim相同寄存器,只是读或写的区别
	unsafe {
		complete_reg.write_volatile(id);
	}
}

//看的中断ID是否pending
pub fn is_pending(id: u32) -> bool {
	let pend = PLIC_PENDING as *const u32;
	let actual_id = 1 << id;
	let pend_ids;
	unsafe {
		pend_ids = pend.read_volatile();
	}
	actual_id & pend_ids != 0
}

//使能target中某个给定ID的中断
//中断ID可查找qemu/include/hw/riscv/virt.h, 如：UART0_IRQ = 10
pub fn enable(id: u32) {
	let enables = PLIC_INT_ENABLE as *mut u32; //32位的寄存器
	let actual_id = 1 << id;
	unsafe {
		enables.write_volatile(enables.read_volatile() | actual_id);
        // 0x0c00_2000 <=~ (1 << 10)
	}
}

//设置中断源的优先级，分0～7级，7是最高级, eg:这里id=10, 表示第10个中断源的设置, prio=1
pub fn set_priority(id: u32, prio: u8) {
	let actual_prio = prio as u32 & 7;
	let prio_reg = PLIC_PRIORITY as *mut u32;
	unsafe {
		prio_reg.add(id as usize).write_volatile(actual_prio); //0x0c000000 + 4 * 10 <= 1 = 1 & 7
	}
}

//设置中断target的全局阀值［0..7]， <= threshold会被屏蔽
pub fn set_threshold(tsh: u8) {
	let actual_tsh = tsh & 7; //使用0b111保留最后三位
	let tsh_reg = PLIC_THRESHOLD as *mut u32;
	unsafe {
		tsh_reg.write_volatile(actual_tsh as u32); // 0x0c20_0000 <= 0 = 0 & 7
	}
}

pub fn handle_interrupt() {
	if let Some(interrupt) = next() {
		match interrupt {
			1..=8 => {
				//virtio::handle_interrupt(interrupt);
			},
			10 => { //UART中断ID是10
				uart::handle_interrupt();
			},
			_ => {
				bare_println!("Unknown external interrupt: {}", interrupt);
			},
		}
		//这将复位pending的中断，允许UART再次中断。
		//否则，UART将被“卡住”
		complete(interrupt);
	}
}


