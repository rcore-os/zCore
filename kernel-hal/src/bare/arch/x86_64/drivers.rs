use alloc::{boxed::Box, sync::Arc};

use zcore_drivers::irq::x86::Apic;
use zcore_drivers::scheme::{IrqScheme, SchemeUpcast};
use zcore_drivers::uart::{BufferedUart, Uart16550Pio};
use zcore_drivers::DeviceResult;

use super::trap;
use crate::drivers::{IRQ, UART};

pub(super) fn init() -> DeviceResult {
    let uart = Arc::new(Uart16550Pio::new(0x3F8));

    Apic::init_local_apic_bsp(crate::mem::phys_to_virt);
    let irq = Arc::new(Apic::new(
        super::special::pc_firmware_tables().0 as usize,
        crate::mem::phys_to_virt,
    ));
    irq.register_device(trap::X86_ISA_IRQ_COM1, uart.clone().upcast())?;
    irq.unmask(trap::X86_ISA_IRQ_COM1)?;
    irq.register_local_apic_handler(
        trap::X86_INT_APIC_TIMER,
        Box::new(|| crate::timer::timer_tick()),
    )?;
    IRQ.init_once_by(irq);
    UART.init_once_by(BufferedUart::new(uart));

    Ok(())
}
