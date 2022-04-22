use crate::arch::timer::set_next_trigger;
use crate::drivers;
use crate::hal_fn::mem::phys_to_virt;
use crate::imp::config::{GIC_BASE, UART_BASE, VIRTIO_BASE};
use alloc::boxed::Box;
use alloc::sync::Arc;
use zcore_drivers::irq::armv8_gic;
use zcore_drivers::scheme::IrqScheme;
use zcore_drivers::uart::{BufferedUart, Pl011Uart};
use zcore_drivers::virtio::{VirtIOHeader, VirtIoBlk};
use zcore_drivers::Device;

pub fn init_early() {
    let uart = Pl011Uart::new(phys_to_virt(UART_BASE));
    let uart = Arc::new(uart);
    let gic = armv8_gic::init(GIC_BASE);
    gic.register_handler(33, Box::new(handle_uart_irq)).ok();
    gic.register_handler(30, Box::new(set_next_trigger)).ok();
    drivers::add_device(Device::Irq(Arc::new(gic)));
    drivers::add_device(Device::Uart(BufferedUart::new(uart)));
}

pub fn init() {
    let virtio_blk = Arc::new(
        VirtIoBlk::new(unsafe { &mut *(phys_to_virt(VIRTIO_BASE) as *mut VirtIOHeader) }).unwrap(),
    );
    drivers::add_device(Device::Block(virtio_blk));
}

fn handle_uart_irq() {
    crate::drivers::all_uart().first_unwrap().handle_irq(0);
}
