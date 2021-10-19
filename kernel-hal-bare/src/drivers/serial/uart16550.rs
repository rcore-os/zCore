//! 16550 serial adapter driver for malta board

use super::SerialDriver;
use crate::arch::serial_put;
use crate::drivers::device_tree::{DEVICE_TREE_INTC, DEVICE_TREE_REGISTRY};
use crate::drivers::IRQ_MANAGER;
use crate::drivers::SERIAL_DRIVERS;
use crate::phys_to_virt;
use kernel_hal::drivers::{DeviceType, Driver, DRIVERS};
use spin::Mutex;
/*
use crate::{
    memory::phys_to_virt,
    util::{read, write},
};
*/
use alloc::{format, string::String, sync::Arc};
use core::fmt::{Arguments, Result, Write};
use device_tree::Node;

use uart_16550::MmioSerialPort;

pub struct SerialPort {
    base: usize,
    ms: Mutex<MmioSerialPort>,
}

impl Driver for SerialPort {
    fn try_handle_interrupt(&self, irq: Option<usize>) -> bool {
        if let Some(c) = self.getchar_option() {
            serial_put(c);
            //super::SERIAL_ACTIVITY.notify_all();
            true
        } else {
            false
        }
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Serial
    }

    fn get_id(&self) -> String {
        format!("com_{}", self.base)
    }
}

impl SerialPort {
    fn new(base: usize) -> SerialPort {
        let mut mmio_serial_port = unsafe { MmioSerialPort::new(base) };
        mmio_serial_port.init();

        SerialPort {
            base,
            ms: Mutex::new(mmio_serial_port),
        }
    }

    pub fn putchar(&self, c: u8) {
        self.ms.lock().send(c);
    }

    pub fn getchar(&mut self) -> u8 {
        let c = self.ms.lock().receive();
        match c {
            255 => b'\0', // null
            c => c,
        }
    }

    pub fn getchar_option(&self) -> Option<u8> {
        let c = self.ms.lock().receive() as isize;
        match c {
            -1 => None,
            c => Some(c as u8),
        }
    }
}

impl SerialDriver for SerialPort {
    fn read(&self) -> u8 {
        self.getchar_option().unwrap_or(0)
    }

    fn write(&self, data: &[u8]) {
        for byte in data {
            self.putchar(*byte);
        }
    }

    fn try_read(&self) -> Option<u8> {
        self.getchar_option()
    }
}

pub fn init_dt(dt: &Node) {
    let addr = dt.prop_usize("reg").unwrap();
    //let shift = dt.prop_u32("reg-shift").unwrap_or(0) as usize;
    let base = phys_to_virt(addr);
    info!("Init uart16550 at {:#x}", base);
    let com = Arc::new(SerialPort::new(base));
    let mut found = false;
    let irq_opt = dt.prop_u32("interrupts").ok().map(|irq| irq as usize);
    DRIVERS.write().push(com.clone());
    SERIAL_DRIVERS.write().push(com.clone());
    if let Ok(intc) = dt.prop_u32("interrupt-parent") {
        if let Some(irq) = irq_opt {
            //PLIC phandle
            if let Some(manager) = DEVICE_TREE_INTC.write().get_mut(&intc) {
                manager.register_local_irq(irq, com.clone());
                info!("registered uart16550 irq {} to PLIC intc", irq);
                info!("Init uart16550 at {:#x}, {:?}", base, dt);
                found = true;
            }
        }
    }
    if !found {
        info!("registered uart16550 to root");
        IRQ_MANAGER.write().register_opt(irq_opt, com);
    }
}

pub fn driver_init() {
    DEVICE_TREE_REGISTRY.write().insert("ns16550a", init_dt);
}
