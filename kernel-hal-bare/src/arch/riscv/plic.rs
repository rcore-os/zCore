use super::interrupt;
use super::uart;
use crate::{putfmt, phys_to_virt};
use super::consts::*;

//通过MMIO地址对平台级中断控制器PLIC的寄存器进行设置
//基于opensbi后一般运行于Hart0 S态，为Target1

//PLIC是async cause 11
//声明claim会清除中断源上的相应pending位。
//即使mip寄存器的MEIP位没有置位, 也可以claim; 声明不被阀值寄存器的设置影响；
//获取按优先级排序后的下一个可用的中断ID
pub fn next() -> Option<u32> {
    let claim_reg = phys_to_virt(PLIC_CLAIM) as *const u32;
    let claim_no;
    unsafe {
        claim_no = claim_reg.read_volatile();
    }
    if claim_no == 0 {
        None //没有可用中断待定
    } else {
        Some(claim_no)
    }
}

//claim时，PLIC不再从该相同设备监听中断
//写claim寄存器，告诉PLIC处理完成该中断
// id 应该来源于next()函数
pub fn complete(id: u32) {
    let complete_reg = phys_to_virt(PLIC_CLAIM) as *mut u32; //和claim相同寄存器,只是读或写的区别
    unsafe {
        complete_reg.write_volatile(id);
    }
}

//看的中断ID是否pending
pub fn is_pending(id: u32) -> bool {
    let pend = phys_to_virt(PLIC_PENDING) as *const u32;
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
    let enables = phys_to_virt(PLIC_INT_ENABLE) as *mut u32; //32位的寄存器
    let actual_id = 1 << id;
    unsafe {
        enables.write_volatile(enables.read_volatile() | actual_id);
        // 0x0c00_2000 <=~ (1 << 10)
    }
}

//设置中断源的优先级，分0～7级，7是最高级, eg:这里id=10, 表示第10个中断源的设置, prio=1
pub fn set_priority(id: u32, prio: u8) {
    let actual_prio = prio as u32 & 7;
    let prio_reg = phys_to_virt(PLIC_PRIORITY) as *mut u32;
    unsafe {
        prio_reg.add(id as usize).write_volatile(actual_prio); //0x0c000000 + 4 * 10 <= 1 = 1 & 7
    }
}

//设置中断target的全局阀值［0..7]， <= threshold会被屏蔽
pub fn set_threshold(tsh: u8) {
    let actual_tsh = tsh & 7; //使用0b111保留最后三位
    let tsh_reg = phys_to_virt(PLIC_THRESHOLD) as *mut u32;
    unsafe {
        tsh_reg.write_volatile(actual_tsh as u32); // 0x0c20_0000 <= 0 = 0 & 7
    }
}

pub fn handle_interrupt() {
    if let Some(interrupt) = next() {
        match interrupt {
            1..=8 => {
                //virtio::handle_interrupt(interrupt);
                bare_println!("plic virtio external interrupt: {}", interrupt);
            }
            UART0_INT_NUM => {
                //UART中断ID是10
                uart::handle_interrupt();

                //换用sbi的方式获取字符
                //interrupt::try_process_serial();
            }
            _ => {
                bare_println!("Unknown external interrupt: {}", interrupt);
            }
        }
        //这将复位pending的中断，允许UART再次中断。
        //否则，UART将被“卡住”
        complete(interrupt);
    }
}
