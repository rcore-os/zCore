use alloc::{boxed::Box, sync::Arc};

use zcore_drivers::irq::x86::Apic;
use zcore_drivers::scheme::{IrqScheme, SchemeUpcast};
use zcore_drivers::uart::{BufferedUart, Uart16550Pio};
use zcore_drivers::{Device, DeviceResult};

use super::trap;
use crate::drivers;

pub(super) fn init() -> DeviceResult {
    let uart = Arc::new(Uart16550Pio::new(0x3F8));
    drivers::add_device(Device::Uart(BufferedUart::new(uart.clone())));

    Apic::init_local_apic_bsp(crate::mem::phys_to_virt);
    let irq = Arc::new(Apic::new(
        super::special::pc_firmware_tables().0 as usize,
        crate::mem::phys_to_virt,
    ));
    irq.register_device(trap::X86_ISA_IRQ_COM1, uart.upcast())?;
    irq.unmask(trap::X86_ISA_IRQ_COM1)?;
    irq.register_local_apic_handler(
        trap::X86_INT_APIC_TIMER,
        Box::new(|| crate::timer::timer_tick()),
    )?;
    drivers::add_device(Device::Irq(irq));

    #[cfg(feature = "graphic")]
    {
        use zcore_drivers::display::UefiDisplay;
        let display = Arc::new(UefiDisplay::new(crate::KCONFIG.display_info));
        crate::drivers::add_device(Device::Display(display.clone()));
        crate::console::init_graphic_console(display);
    }

    Ok(())
}
