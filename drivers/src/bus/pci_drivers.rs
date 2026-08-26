//! Registro modular de drivers PCI.
//!
//! Cada driver implementa el trait [`PciDriver`] y se registra en [`init_all`].
//! La función [`probe_pci_device`] itera el registro para encontrar el driver
//! adecuado para cada dispositivo PCI detectado.

use alloc::sync::Arc;
use alloc::vec::Vec;

use lock::Mutex;
use pci::PCIDevice;

use crate::builder::IoMapper;
use crate::{Device, DeviceResult};

/// Interfaz que debe implementar cada driver PCI modular.
pub trait PciDriver: Send + Sync {
    /// Nombre descriptivo del driver (solo para log/debug).
    fn name(&self) -> &str;

    /// Devuelve `true` si este driver gestiona el par (vendor_id, device_id).
    fn matched(&self, vendor_id: u16, device_id: u16) -> bool;

    /// Devuelve `true` si este driver gestiona el dispositivo dado.
    ///
    /// La implementación por defecto delega en [`matched`](PciDriver::matched)
    /// pasando los ids de `dev`. Los drivers que necesiten inspeccionar
    /// class/subclass/prog_if deben sobreescribir este método.
    fn matched_dev(&self, dev: &PCIDevice) -> bool {
        self.matched(dev.id.vendor_id, dev.id.device_id)
    }

    /// Inicializa el dispositivo y devuelve el [`Device`] creado.
    fn init(
        &self,
        dev: &PCIDevice,
        mapper: &Option<Arc<dyn IoMapper>>,
        irq: Option<usize>,
    ) -> DeviceResult<Device>;
}

// ——— Registro global ———

static DRIVERS: Mutex<Vec<&'static (dyn PciDriver + Send + Sync)>> = Mutex::new(Vec::new());

fn register(driver: &'static (dyn PciDriver + Send + Sync)) {
    DRIVERS.lock().push(driver);
}

/// Registra todos los drivers PCI conocidos.
///
/// Debe llamarse una vez en tiempo de inicialización, antes de enumerar el bus PCI.
pub fn init_all() {
    // NVMe
    register(&crate::nvme::interface::NvmeDriverPci);

    // AHCI / SATA
    register(&crate::ata::ahci::AhciDriverPci);

    // Ethernet Intel e1000 / e1000e
    register(&crate::net::e1000::E1000DriverPci);
    register(&crate::net::e1000e::E1000eDriverPci);

    // GPU Nvidia (scaffolding) — x86_64-only (nvidia-rm-sys shim).
    #[cfg(target_arch = "x86_64")]
    register(&crate::display::NvidiaGpuDriverPci);

    // HD Audio (PCH onboard + NVIDIA GPU HDMI audio functions)
    register(&crate::audio::hda::HdaDriverPci);

    // xHCI USB HID (teclado/ratón/tablet)
    #[cfg(all(
        any(feature = "xhci-usb-hid", feature = "legacy-usb-hid"),
        target_arch = "x86_64",
        not(feature = "mock"),
        not(feature = "no-pci")
    ))]
    register(&crate::usb::xhci_hid::XhciDriverPci);
}

/// Intenta inicializar `dev` con el primer driver del registro que lo soporte.
///
/// Devuelve `Err(DeviceError::NotSupported)` si ningún driver coincide.
pub fn probe_pci_device(
    dev: &PCIDevice,
    mapper: &Option<Arc<dyn IoMapper>>,
    irq: Option<usize>,
) -> DeviceResult<Device> {
    let drivers = DRIVERS.lock();
    for drv in drivers.iter() {
        if drv.matched_dev(dev) {
            match drv.init(dev, mapper, irq) {
                ok @ Ok(_) => {
                    info!("[pci] {} inicializado correctamente", drv.name());
                    return ok;
                }
                Err(e) => {
                    warn!("[pci] driver '{}' falló: {:?}", drv.name(), e);
                }
            }
        }
    }
    Err(crate::DeviceError::NotSupported)
}
