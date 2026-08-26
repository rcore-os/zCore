use alloc::sync::Arc;

#[cfg(feature = "graphic")]
use alloc::format;

use zcore_drivers::irq::x86::Apic;
use zcore_drivers::scheme::{IrqScheme, SchemeUpcast};
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

fn boot_progress(p: u32) {
    #[cfg(feature = "graphic")]
    crate::console::early_progress_bar(p);
    #[cfg(not(feature = "graphic"))]
    let _ = p;
}

pub(super) fn init() -> DeviceResult {
    boot_progress(81);
    zcore_drivers::init();
    boot_progress(82);
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

    // PS/2 Keyboard and Mouse initialization and registration
    let ps2_input = Arc::new(zcore_drivers::input::Ps2Input::new());
    irq.register_device(trap::X86_ISA_IRQ_KEYBOARD, ps2_input.clone().upcast())?;
    irq.unmask(trap::X86_ISA_IRQ_KEYBOARD)?;
    irq.register_device(trap::X86_ISA_IRQ_MOUSE, ps2_input.clone().upcast())?;
    irq.unmask(trap::X86_ISA_IRQ_MOUSE)?;
    drivers::add_device(Device::Input(ps2_input));

    use x2apic::lapic::{TimerDivide, TimerMode};

    irq.register_local_apic_handler(trap::X86_INT_APIC_TIMER, Arc::new(super::trap::super_timer))?;
    // IPI vector: 0xf3 = X86_INT_LOCAL_APIC_BASE + 3
    irq.register_local_apic_handler(
        0xf3,
        Arc::new(|| {
            // Either a TLB shootdown (queue entries: invlpg/flush + publish the
            // satisfied generation) or a pure reschedule wake (empty queue: the
            // interrupt already broke `hlt`, nothing else to do).
            crate::common::ipi::tlb_shootdown_ack();
        }),
    )?;

    if Apic::local_apic_ready() {
        // SAFETY: called once on BSP during primary_init
        let lapic = Apic::local_apic();
        lapic.set_timer_mode(TimerMode::Periodic);
        lapic.set_timer_divide(TimerDivide::Div256); // indeed it is Div1, the name is confusing.
        let cycles =
            super::cpu::cpu_frequency() as u64 * 1_000_000 / super::super::timer::TICKS_PER_SEC;
        lapic.set_timer_initial(cycles as u32);
        lapic.disable_timer();
    } else {
        crate::klog_warn!("[drivers] LAPIC unavailable — APIC timer left disabled");
    }

    #[cfg(all(not(feature = "no-pci"), feature = "xhci-usb-hid"))]
    {
        use zcore_drivers::usb::xhci_hid;
        let irq_apic: Arc<dyn zcore_drivers::scheme::IrqScheme> = irq.clone();
        xhci_hid::pci_set_irq_host(irq_apic);
    }

    drivers::add_device(Device::Irq(irq.clone()));
    boot_progress(83);

    // Register the UEFI GOP framebuffer as a Display device EARLY and
    // UNCONDITIONALLY, before the fallible PCI / nvidia bring-up below.
    //
    // linux-object's software-KMS path (drm.rs `software_kms_active`) is what
    // actually drives the compositor: it blits wlroots' dumb buffer into the
    // GOP framebuffer via `scanout`. That path is gated purely on
    // `drivers::all_display().first().is_some()`. If we only registered the
    // display in the graphics-console block further down, it would run AFTER
    // `pci::init(...)?` (which can return Err and bail out of `init`) and
    // behind an else-arm, so a fallible/reordered PCI path could leave
    // `all_display()` empty. In that state GETRESOURCES hands wlroots the
    // nvidia stub CRTC (whose `page_flip` is a no-op) and the screen stays
    // black. Registering here guarantees a real scanout target regardless of
    // how PCI / nvidia bring-up goes.
    #[cfg(feature = "graphic")]
    {
        use crate::KCONFIG;
        use zcore_drivers::display::UefiDisplay;
        use zcore_drivers::prelude::{ColorFormat, DisplayInfo};

        let (width, height) = KCONFIG.fb_mode.resolution();
        let stride = KCONFIG.fb_mode.stride();

        // Publish the boot framebuffer geometry + real panel EDID to the
        // display layer up front so the DRM connector / blit code sees them
        // regardless of the PCI path (also covers the `no-pci` build).
        zcore_drivers::display::set_boot_fb_info(
            KCONFIG.fb_addr,
            width as u32,
            height as u32,
            (stride * 4) as u32,
        );
        zcore_drivers::display::set_boot_edid(&KCONFIG.fb_edid, KCONFIG.fb_edid_size);

        if KCONFIG.fb_addr != 0
            && width != 0
            && height != 0
            && crate::drivers::all_display().first().is_none()
        {
            let display = Arc::new(UefiDisplay::new(DisplayInfo {
                width: width as _,
                height: height as _,
                pitch: (stride * 4) as u32,
                format: ColorFormat::ARGB8888,
                fb_base_vaddr: crate::mem::phys_to_virt(KCONFIG.fb_addr as usize),
                fb_size: KCONFIG.fb_size as usize,
            }));
            crate::drivers::add_device(Device::Display(display));
        }
    }

    #[cfg(not(feature = "no-pci"))]
    {
        // PCI scan
        use crate::vm::{GenericPageTable, PageTable};
        use crate::{CachePolicy, MMUFlags, PhysAddr, VirtAddr};
        use zcore_drivers::builder::IoMapper;
        use zcore_drivers::bus::pci;

        struct IoMapperImpl;
        impl IoMapper for IoMapperImpl {
            fn query_or_map(&self, paddr: PhysAddr, size: usize) -> Option<VirtAddr> {
                let vaddr = crate::mem::phys_to_virt(paddr);
                let mut pt = PageTable::from_current();

                if let Ok((paddr_mapped, _, _)) = pt.query(vaddr) {
                    if paddr_mapped == paddr {
                        return Some(vaddr);
                    }
                }

                let size = (size + 0xfff) & !0xfff;
                let flags = MMUFlags::READ
                    | MMUFlags::WRITE
                    | MMUFlags::DEVICE
                    | MMUFlags::from_bits_truncate(CachePolicy::UncachedDevice as usize);

                // debug!, not warn!: fires for every PCI BAR of every device at
                // boot; at LOG=warn each line pays the per-byte UART spin.
                debug!(
                    "[xhci] Mapeando BAR PCI en PT kernel: {:#x} -> {:#x} (size: {:#x})",
                    paddr, vaddr, size
                );
                if let Err(e) = pt.map_cont(vaddr, size, paddr, flags) {
                    crate::klog_err!("[xhci] failed to map PCI BAR: {:?}", e);
                    return None;
                }

                core::mem::forget(pt);
                Some(vaddr)
            }
        }

        // Boot framebuffer geometry + panel EDID were already published to the
        // display layer in the early graphic block above (before this fallible
        // PCI init), so display drivers already have native-resolution info.

        boot_progress(84);
        let pci_devs = pci::init(Some(Arc::new(IoMapperImpl)))?;
        // Do NOT drain deferred jobs here — e1000e PHY/MAC init runs in the idle
        // loop / NIC poll path so boot progress is not blocked at 84% or 87%.
        boot_progress(87);
        for d in pci_devs.into_iter() {
            drivers::add_device(d);
        }

        // Finish deferred NIC work without starving HID at login.
        for _ in 0..2 {
            zcore_drivers::utils::deferred_job::drain_deferred_jobs_max(1);
        }

        // Finish MSI registrations for USB
        #[cfg(feature = "xhci-usb-hid")]
        {
            use zcore_drivers::usb::xhci_hid;
            xhci_hid::pci_set_irq_host(irq.clone());
            let _ = xhci_hid::pci_finish_msi_registrations();
        }

        // Finish MSI registrations for Net
        {
            use zcore_drivers::net;
            net::pci_set_irq_host(irq.clone());
            let _ = net::pci_finish_msi_registrations();
        }
    }

    boot_progress(88);

    #[cfg(feature = "graphic")]
    let graphics_console_note = {
        use alloc::string::String;
        if let Some(display) = crate::drivers::all_display().first() {
            crate::console::init_graphic_console(display.clone());
            let _ = display.need_flush();
            let info = display.info();
            Some(format!("{} {}x{}", display.name(), info.width, info.height))
        } else {
            use crate::KCONFIG;
            use zcore_drivers::display::UefiDisplay;
            use zcore_drivers::prelude::{ColorFormat, DisplayInfo};

            let (width, height) = KCONFIG.fb_mode.resolution();
            let stride = KCONFIG.fb_mode.stride();
            if KCONFIG.fb_addr == 0 || width == 0 || height == 0 {
                crate::klog_warn!(
                    "[drivers] no framebuffer from bootloader (fb_addr={:#x}, {}x{}) — skipping graphic console",
                    KCONFIG.fb_addr, width, height
                );
                Some(String::from("unavailable (no bootloader framebuffer)"))
            } else {
                let display = Arc::new(UefiDisplay::new(DisplayInfo {
                    width: width as _,
                    height: height as _,
                    pitch: (stride * 4) as u32,
                    format: ColorFormat::ARGB8888,
                    fb_base_vaddr: crate::mem::phys_to_virt(KCONFIG.fb_addr as usize),
                    fb_size: KCONFIG.fb_size as usize,
                }));
                crate::drivers::add_device(Device::Display(display.clone()));
                crate::console::init_graphic_console(display.clone());
                Some(format!("uefi-gop {}x{}", width, height))
            }
        }
    };

    #[cfg(not(feature = "graphic"))]
    let graphics_console_note: Option<alloc::string::String> = None;

    drivers::klog_graphics_device_summary(graphics_console_note.as_deref());

    use crate::net;
    net::init();

    crate::klog_info!("Eclipse: drivers init complete");
    Ok(())
}
