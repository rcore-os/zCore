//use super::consts::UART_BASE;
use crate::arch::serial_put;
use crate::drivers::device_tree::{DEVICE_TREE_INTC, DEVICE_TREE_REGISTRY};
use crate::drivers::{SerialDriver, IRQ_MANAGER, SERIAL_DRIVERS};
use crate::{phys_to_virt, putfmt};
use alloc::{format, string::String, sync::Arc};
use core::convert::TryInto;
use core::fmt::{Error, Write};
use device_tree::Node;
use kernel_hal::drivers::{DeviceType, Driver, DRIVERS};

pub struct Uart {
    base_address: usize,
}

// 结构体Uart的实现块
impl Uart {
    pub fn new(base_address: usize) -> Self {
        Uart { base_address }
    }

    #[cfg(not(feature = "board_d1"))]
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

    #[cfg(feature = "board_d1")]
    pub fn simple_init(&mut self) {
        let ptr = self.base_address as *mut u32;
        unsafe {
            // Enable FIFO; (base + 2)
            ptr.add(2).write_volatile(0x7);

            // MODEM Ctrl; (base + 4)
            ptr.add(4).write_volatile(0x3);

            //D1 ALLWINNER的uart中断使能
            // D1 UART_IER offset = 0x4
            //
            // Enable interrupts; (base + 1)
            ptr.add(1).write_volatile(0x1);
        }
    }

    pub fn get(&self) -> Option<u8> {
        #[cfg(not(feature = "board_d1"))]
        let ptr = self.base_address as *const u8;
        #[cfg(feature = "board_d1")]
        let ptr = self.base_address as *const u32;

        unsafe {
            //查看LSR的DR位为1则有数据
            if ptr.add(5).read_volatile() & 0b1 == 0 {
                None
            } else {
                Some((ptr.add(0).read_volatile() & 0xff) as u8)
            }
        }
    }

    pub fn put(&self, c: u8) {
        let ptr = self.base_address as *mut u8;
        unsafe {
            //此时transmitter empty
            ptr.add(0).write_volatile(c);
        }
    }
}

impl Driver for Uart {
    fn try_handle_interrupt(&self, irq: Option<usize>) -> bool {
        if let Some(c) = self.get() {
            let c = c & 0xff;
            serial_put(c);

            true
        } else {
            false
        }
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Serial
    }

    fn get_id(&self) -> String {
        format!("uart_{}", self.base_address)
    }
}

impl SerialDriver for Uart {
    fn read(&self) -> u8 {
        self.get().unwrap_or(0)
    }

    fn write(&self, data: &[u8]) {
        for byte in data {
            self.put(*byte);
        }
    }

    fn try_read(&self) -> Option<u8> {
        self.get()
    }
}

// 需要实现的write_str()重要函数
impl Write for Uart {
    fn write_str(&mut self, out: &str) -> Result<(), Error> {
        for c in out.bytes() {
            self.put(c);
        }
        Ok(())
    }
}

/*
pub fn handle_interrupt() {
    let mut my_uart = Uart::new(phys_to_virt(UART_BASE));
    if let Some(c) = my_uart.get() {
        let c = c & 0xff;
        //CONSOLE
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
*/

pub fn init_dt(dt: &Node) {
    let addr = dt.prop_usize("reg").unwrap();
    let base = phys_to_virt(addr);
    info!("Init Uart at {:#x}", base);

    let mut us = Uart::new(base);
    us.simple_init();

    let com = Arc::new(us);
    let mut found = false;
    let irq_opt = dt.prop_u32("interrupts").ok().map(|irq| irq as usize);
    DRIVERS.write().push(com.clone());
    SERIAL_DRIVERS.write().push(com.clone());

    if let Ok(intc) = dt.prop_u32("interrupt-parent") {
        if let Some(irq) = irq_opt {
            if let Some(manager) = DEVICE_TREE_INTC.write().get_mut(&intc) {
                manager.register_local_irq(irq, com.clone());
                info!("Registered Uart irq {} to PLIC intc", irq);
                info!("Init Uart at {:#x}, {:?}", base, dt);
                found = true;
            }
        }
    }
    if !found {
        info!("Registered Uart to root");
        IRQ_MANAGER.write().register_opt(irq_opt, com);
    }
}

pub fn driver_init() {
    DEVICE_TREE_REGISTRY.write().insert("ns16550a", init_dt);
}
