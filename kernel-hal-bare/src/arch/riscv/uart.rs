use core::convert::TryInto;
use core::fmt::{Error, Write};
use crate::{putfmt, KERNEL_OFFSET};

//use crate::console::push_stdin;

pub struct Uart {
	base_address: usize,
}

// 结构体Uart的实现块
impl Uart {
	pub fn new(base_address: usize) -> Self {
		Uart {
			base_address
		}
	}
/*
uart初始化
设置字长为8-bits (LCR[1:0])
使能先进先出FIFOs (FCR[0])
使能接受中断(IER[0]), 在这只使用轮询的方式而不用中断

*/
pub fn init(&mut self) {
	let ptr = self.base_address as *mut u8;
	unsafe {
		// LCR at base_address + 3
		// 置位     bit 0      bit 1
		let lcr = (1 << 0) | (1 << 1);
		ptr.add(3).write_volatile(lcr);

		// FCR at offset 2
		ptr.add(2).write_volatile(1 << 0);

		//IER at offset 1
		ptr.add(1).write_volatile(1 << 0);

		// 设置波特率，除子，取整等
		// 2.729 MHz (22,729,000 cycles per second) --> 波特率 2400 (BAUD)

		// 根据NS16550a规格说明书计算出divisor
		// divisor = ceil( (clock_hz) / (baud_sps x 16) )
		// divisor = ceil( 22_729_000 / (2400 x 16) ) = ceil( 591.901 ) = 592


		// divisor寄存器是16 bits
		let divisor: u16 = 592;
		//let divisor_least: u8 = divisor & 0xff;
		//let divisor_most:  u8 = divisor >> 8;
		let divisor_least: u8 = (divisor & 0xff).try_into().unwrap();
		let divisor_most:  u8 = (divisor >> 8).try_into().unwrap();

		// DLL和DLM会与其它寄存器共用基地址，需要设置DLAB来切换选择寄存器
		// LCR base_address + 3, DLAB = 1
		ptr.add(3).write_volatile(lcr | 1 << 7);

		//写DLL和DLM来设置波特率, 把频率22.729 MHz的时钟划分为每秒2400个信号
		ptr.add(0).write_volatile(divisor_least);
		ptr.add(1).write_volatile(divisor_most);

		// 设置后不需要再动了, 清空DLAB
		ptr.add(3).write_volatile(lcr);
	}
}

pub fn simple_init(&mut self) {
	let ptr = self.base_address as *mut u8;
	unsafe {
        // Enable FIFO; (base + 2)
        ptr.add(2).write_volatile(0xC7);

        // MODEM Ctrl; (base + 4)
        ptr.add(4).write_volatile(0x0B);

        // Enable interrupts; (base + 1)
        ptr.add(1).write_volatile(0x01);
    }
}

pub fn get(&mut self) -> Option<u8> {
	let ptr = self.base_address as *mut u8;
	unsafe {
		//查看LCR, DR位为1则有数据
		if ptr.add(5).read_volatile() & 0b1 == 0 {
			None
		} else {
			Some(ptr.add(0).read_volatile())
		}
	}

}

pub fn put(&mut self, c: u8) {
	let ptr = self.base_address as *mut u8;
	unsafe {
		//此时transmitter empty
		ptr.add(0).write_volatile(c);
	}
}

}

// 需要实现的write_str()重要函数
impl Write for Uart {
	fn write_str(&mut self, out: &str) -> Result<(), Error> {
		for c in out.bytes(){
			self.put(c);
		}
		Ok(())
	}
}

/*
fn unsafe mmio_write(address: usize, offset: usize, value: u8) {
	//write_volatile() 是 *mut raw 的成员；
	//new_pointer = old_pointer + sizeof(pointer_type) * offset
	//也可使用reg.offset

	let reg = address as *mut u8;
	reg.add(offset).write_volatile(value);
}

fn unsafe mmio_read(address: usize, offset: usize, value: u8) -> u8 {

	let reg = address as *mut u8;

	//读取8 bits
	reg.add(offset).read_volatile(value) //无分号可直接返回值
}
*/

pub fn handle_interrupt() {
	let mut my_uart = Uart::new(0x1000_0000 + KERNEL_OFFSET);
	if let Some(c) = my_uart.get() {
		//CONSOLE
		//push_stdin(c);
        super::serial_put(c);

        /*
         * 因serial_write()已可以被回调输出了，这里则不再需要了
		match c {
			0x7f => { //0x8 [backspace] ; 而实际qemu运行，[backspace]键输出0x7f, 表示del
				bare_print!("{} {}", 8 as char, 8 as char);
			},
			10 | 13 => { // 新行或回车
				bare_println!();
			},
			_ => {
				bare_print!("{}", c as char);
			},
		}
        */
	}
}

