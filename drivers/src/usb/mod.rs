//! USB (xHCI) — soporte experimental para HID.
//!
//! El núcleo de bajo nivel vive en [`xhci_hid`]. Tras el escaneo PCI, `kernel-hal`
//! debe llamar a [`xhci_hid::pci_finish_msi_registrations`] para enlazar MSI.
//! En bare x86, [`xhci_hid::poll`] se invoca desde el tick del timer como respaldo
//! (QEMU / IRQ perdidos), además del MSI en [`XhciUsbHid::handle_irq`].

#![allow(dead_code)]

pub mod xhci_hid;
