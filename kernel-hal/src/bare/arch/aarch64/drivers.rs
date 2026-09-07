use crate::arch::timer::set_next_trigger;
use crate::drivers;
use crate::hal_fn::mem::phys_to_virt;
use crate::imp::config::VIRTIO_BASE;
use crate::KCONFIG;
use alloc::boxed::Box;
use alloc::sync::Arc;
use zcore_drivers::irq::gic_400;
use zcore_drivers::scheme::IrqScheme;
use zcore_drivers::uart::{BufferedUart, Pl011Uart};
use zcore_drivers::virtio::{VirtIOHeader, VirtIoBlk};
use zcore_drivers::Device;

pub fn init_early() {
    let uart = Pl011Uart::new(phys_to_virt(KCONFIG.uart_base));
    let uart = Arc::new(uart);
    let gic = gic_400::init(
        phys_to_virt(KCONFIG.gic_base + 0x1_0000),
        phys_to_virt(KCONFIG.gic_base),
    );
    gic.irq_enable(30);
    gic.irq_enable(33);
    gic.register_handler(33, Box::new(handle_uart_irq)).ok();
    gic.register_handler(30, Box::new(handle_timer_irq)).ok();
    drivers::add_device(Device::Irq(Arc::new(gic)));
    drivers::add_device(Device::Uart(BufferedUart::new(uart)));
}

pub fn init() {
    // The QEMU virt machine exposes a bank of virtio-mmio transports, and the
    // firmware boot disk is not guaranteed to occupy the first one.  A block
    // device is optional (the Zircon ZBI is linked into the kernel), so do not
    // abort boot when this slot is empty.
    if let Ok(virtio_blk) =
        VirtIoBlk::new(unsafe { &mut *(phys_to_virt(VIRTIO_BASE) as *mut VirtIOHeader) })
    {
        drivers::add_device(Device::Block(Arc::new(virtio_blk)));
    }
}

fn handle_timer_irq() {
    set_next_trigger();
    crate::timer::timer_tick();
}

fn handle_uart_irq() {
    crate::drivers::all_uart().first_unwrap().handle_irq(0);
}
