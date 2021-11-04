use alloc::{boxed::Box, sync::Arc};

use zcore_drivers::irq::x86::Apic;
use zcore_drivers::scheme::IrqScheme;
use zcore_drivers::uart::{BufferedUart, Uart16550Pio};
use zcore_drivers::{Device, DeviceResult};

use super::trap;
use crate::drivers;

pub(super) fn init_early() -> DeviceResult {
    let uart = Arc::new(Uart16550Pio::new(0x3F8));
    drivers::add_device(Device::Uart(BufferedUart::new(uart)));
    Ok(())
}

pub(super) fn init() -> DeviceResult {
    Apic::init_local_apic_bsp(crate::mem::phys_to_virt);
    let irq = Arc::new(Apic::new(
        super::special::pc_firmware_tables().0 as usize,
        crate::mem::phys_to_virt,
    ));
    irq.register_device(
        trap::X86_ISA_IRQ_COM1,
        drivers::all_uart().first_unwrap().upcast(),
    )?;
    irq.unmask(trap::X86_ISA_IRQ_COM1)?;
    irq.register_local_apic_handler(trap::X86_INT_APIC_TIMER, Box::new(crate::timer::timer_tick))?;
    drivers::add_device(Device::Irq(irq));

    #[cfg(feature = "graphic")]
    {
        use crate::KCONFIG;
        use zcore_drivers::display::UefiDisplay;
        use zcore_drivers::prelude::{ColorFormat, DisplayInfo};

        let (width, height) = KCONFIG.fb_mode.resolution();
        let display = Arc::new(UefiDisplay::new(DisplayInfo {
            width: width as _,
            height: height as _,
            format: ColorFormat::ARGB8888, // uefi::proto::console::gop::PixelFormat::Bgr
            fb_base_vaddr: crate::mem::phys_to_virt(KCONFIG.fb_addr as usize),
            fb_size: KCONFIG.fb_size as usize,
        }));
        crate::drivers::add_device(Device::Display(display.clone()));
        crate::console::init_graphic_console(display);
    }

    info!("Drivers init end.");
    Ok(())
}
