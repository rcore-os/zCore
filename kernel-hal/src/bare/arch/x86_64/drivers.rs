use alloc::{boxed::Box, sync::Arc};

use zcore_drivers::irq::x86::Apic;
use zcore_drivers::scheme::IrqScheme;
use zcore_drivers::uart::{BufferedUart, Uart16550Pmio};
use zcore_drivers::{Device, DeviceResult};

use super::trap;
use crate::drivers;

pub(super) fn init_early() -> DeviceResult {
    let uart = Arc::new(Uart16550Pmio::new(0x3F8));
    drivers::add_device(Device::Uart(BufferedUart::new(uart)));
    let uart = Arc::new(Uart16550Pmio::new(0x2F8));
    drivers::add_device(Device::Uart(BufferedUart::new(uart)));
    Ok(())
}

pub(super) fn init() -> DeviceResult {
    Apic::init_local_apic_bsp(crate::mem::phys_to_virt);
    let irq = Arc::new(Apic::new(
        super::special::pc_firmware_tables().0 as usize,
        crate::mem::phys_to_virt,
    ));
    let uarts = drivers::all_uart();
    if let Some(u) = uarts.try_get(0) {
        irq.register_device(trap::X86_ISA_IRQ_COM1, u.clone().upcast())?;
        irq.unmask(trap::X86_ISA_IRQ_COM1)?;

        if let Some(u) = uarts.try_get(1) {
            irq.register_device(trap::X86_ISA_IRQ_COM2, u.clone().upcast())?;
            irq.unmask(trap::X86_ISA_IRQ_COM2)?;
        }
    }

    use x2apic::lapic::{TimerDivide, TimerMode};

    irq.register_local_apic_handler(trap::X86_INT_APIC_TIMER, Box::new(super::trap::super_timer))?;

    // SAFETY: this will be called once and only once for every core
    Apic::local_apic().set_timer_mode(TimerMode::Periodic);
    Apic::local_apic().set_timer_divide(TimerDivide::Div256); // indeed it is Div1, the name is confusing.
    let cycles =
        super::cpu::cpu_frequency() as u64 * 1_000_000 / super::super::timer::TICKS_PER_SEC;
    Apic::local_apic().set_timer_initial(cycles as u32);
    Apic::local_apic().disable_timer();

    drivers::add_device(Device::Irq(irq));

    #[cfg(not(feature = "no-pci"))]
    {
        // PCI scan
        use zcore_drivers::bus::pci;
        let pci_devs = pci::init(None)?;
        for d in pci_devs.into_iter() {
            drivers::add_device(d);
        }
    }

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

    #[cfg(feature = "loopback")]
    {
        use crate::net;
        net::init();
    }

    info!("Drivers init end.");
    Ok(())
}
