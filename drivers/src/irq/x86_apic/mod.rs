mod consts;
mod ioapic;
mod lapic;

use core::ops::Range;

use spin::Mutex;

use self::consts::{X86_INT_BASE, X86_INT_LOCAL_APIC_BASE, X86_INT_MAX};
use self::ioapic::{IoApic, IoApicList};
use self::lapic::LocalApic;
use crate::scheme::{IrqHandler, IrqScheme, Scheme};
use crate::{utils::IrqManager, DeviceError, DeviceResult};

const IRQ_RANGE: Range<usize> = X86_INT_BASE..X86_INT_MAX + 1;

type Phys2VirtFn = fn(usize) -> usize;

pub struct Apic {
    ioapic_list: IoApicList,
    manager: Mutex<IrqManager<256>>,
}

impl Apic {
    pub fn new(acpi_rsdp: usize, phys_to_virt: Phys2VirtFn) -> Self {
        Self {
            manager: Mutex::new(IrqManager::new(IRQ_RANGE)),
            ioapic_list: IoApicList::new(acpi_rsdp, phys_to_virt),
        }
    }

    fn with_ioapic<F>(&self, gsi: u32, op: F) -> DeviceResult
    where
        F: FnOnce(&IoApic) -> DeviceResult,
    {
        if let Some(apic) = self.ioapic_list.find(gsi) {
            op(apic)
        } else {
            error!(
                "cannot find IOAPIC for global system interrupt number {}",
                gsi
            );
            Err(DeviceError::InvalidParam)
        }
    }

    pub fn init_local_apic_bsp(phys_to_virt: Phys2VirtFn) {
        unsafe { self::lapic::init_bsp(phys_to_virt) }
    }

    pub fn init_local_apic_ap() {
        unsafe { self::lapic::init_ap() }
    }

    pub fn local_apic<'a>() -> &'a mut LocalApic {
        unsafe { self::lapic::get_local_apic() }
    }

    pub fn register_local_apic_handler(&self, vector: usize, handler: IrqHandler) -> DeviceResult {
        if vector >= X86_INT_LOCAL_APIC_BASE {
            self.manager.lock().register_handler(vector, handler)?;
            Ok(())
        } else {
            error!("invalid local APIC interrupt vector {}", vector);
            Err(DeviceError::InvalidParam)
        }
    }
}

impl Scheme for Apic {
    fn handle_irq(&self, vector: usize) {
        if self.manager.lock().handle(vector).is_err() {
            warn!("no registered handler for interrupt vector {}!", vector);
        }
        Self::local_apic().eoi();
    }
}

impl IrqScheme for Apic {
    fn mask(&self, gsi: usize) {
        self.with_ioapic(gsi as _, |apic| Ok(apic.toggle(gsi as _, false)))
            .ok();
    }

    fn unmask(&self, gsi: usize) {
        self.with_ioapic(gsi as _, |apic| Ok(apic.toggle(gsi as _, true)))
            .ok();
    }

    fn register_handler(&self, gsi: usize, handler: IrqHandler) -> DeviceResult {
        let gsi = gsi as u32;
        self.with_ioapic(gsi, |apic| {
            let vector = apic.get_vector(gsi) as _; // if not mapped, allocate an available vector by `register_handler()`.
            let vector = self.manager.lock().register_handler(vector, handler)? as u8;
            apic.map_vector(gsi, vector);
            apic.toggle(gsi, true);
            Ok(())
        })
    }
}
