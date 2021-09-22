use alloc::boxed::Box;

use zcore_drivers::irq::x86::Apic;
use zcore_drivers::scheme::{EventListener, IrqScheme};
use zcore_drivers::uart::Uart16550Pio;
use zcore_drivers::DeviceResult;

use super::trap;
use crate::drivers::{IRQ, UART};

pub(super) fn init() -> DeviceResult {
    UART.init_by(Box::new(EventListener::new(Uart16550Pio::new(0x3F8))));

    Apic::init_local_apic_bsp(crate::mem::phys_to_virt);
    let irq = Box::new(Apic::new(
        super::special::pc_firmware_tables().0 as usize,
        crate::mem::phys_to_virt,
    ));
    irq.register_device(trap::X86_ISA_IRQ_COM1, UART.as_scheme())?;
    irq.register_local_apic_handler(
        trap::X86_INT_APIC_TIMER,
        Box::new(|_| crate::timer::timer_tick()),
    )?;
    IRQ.init_by(irq);

    Ok(())
}
