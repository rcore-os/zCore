use crate::drivers;
use zcore_drivers::uart::Pl011Uart;
use zcore_drivers::Device;
use alloc::sync::Arc;
use crate::imp::config::UART_BASE;
use zcore_drivers::uart::BufferedUart;
use crate::hal_fn::mem::phys_to_virt;
use zcore_drivers::virtio::{VirtIoBlk, VirtIOHeader};
use crate::imp::config::VIRTIO_BASE;

pub fn init_early() {
    let uart = Arc::new(Pl011Uart::new(phys_to_virt(UART_BASE)));
    drivers::add_device(Device::Uart(BufferedUart::new(uart)));
}

pub fn init() {
    let virtio_blk = Arc::new(VirtIoBlk::new(unsafe { &mut *(phys_to_virt(VIRTIO_BASE) as *mut VirtIOHeader) }).unwrap());
    drivers::add_device(Device::Block(virtio_blk));
}
