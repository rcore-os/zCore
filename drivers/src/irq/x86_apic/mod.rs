mod consts;
mod ioapic;
mod lapic;

use core::ops::Range;

use spin::Mutex;

use self::consts::{X86_INT_BASE, X86_INT_LOCAL_APIC_BASE};
use self::ioapic::{IoApic, IoApicList};
use self::lapic::LocalApic;
use crate::prelude::{IrqHandler, IrqPolarity, IrqTriggerMode};
use crate::scheme::{IrqScheme, Scheme};
use crate::{utils::IrqManager, DeviceError, DeviceResult, PhysAddr, VirtAddr};

const IOAPIC_IRQ_RANGE: Range<usize> = X86_INT_BASE..X86_INT_LOCAL_APIC_BASE;
const LAPIC_IRQ_RANGE: Range<usize> = 0..16;

type Phys2VirtFn = fn(paddr: PhysAddr) -> VirtAddr;

pub struct Apic {
    ioapic_list: IoApicList,
    manager_ioapic: Mutex<IrqManager<256>>,
    manager_lapic: Mutex<IrqManager<16>>,
}

impl Apic {
    pub fn new(acpi_rsdp: usize, phys_to_virt: Phys2VirtFn) -> Self {
        Self {
            ioapic_list: IoApicList::new(acpi_rsdp, phys_to_virt),
            manager_ioapic: Mutex::new(IrqManager::new(IOAPIC_IRQ_RANGE)),
            manager_lapic: Mutex::new(IrqManager::new(LAPIC_IRQ_RANGE)),
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
        unsafe { LocalApic::init_bsp(phys_to_virt) }
    }

    pub fn init_local_apic_ap() {
        unsafe { LocalApic::init_ap() }
    }

    pub fn local_apic<'a>() -> &'a mut LocalApic {
        unsafe { LocalApic::get() }
    }

    pub fn register_local_apic_handler(&self, vector: usize, handler: IrqHandler) -> DeviceResult {
        if vector >= X86_INT_LOCAL_APIC_BASE {
            self.manager_lapic
                .lock()
                .register_handler(vector - X86_INT_LOCAL_APIC_BASE, handler)?;
            Ok(())
        } else {
            error!("invalid local APIC interrupt vector {}", vector);
            Err(DeviceError::InvalidParam)
        }
    }
}

impl Scheme for Apic {
    fn name(&self) -> &str {
        "x86-apic"
    }

    fn handle_irq(&self, vector: usize) {
        Self::local_apic().eoi();
        let res = if vector >= X86_INT_LOCAL_APIC_BASE {
            self.manager_lapic
                .lock()
                .handle(vector - X86_INT_LOCAL_APIC_BASE)
        } else {
            self.manager_ioapic.lock().handle(vector)
        };
        if res.is_err() {
            warn!("no registered handler for interrupt vector {}!", vector);
        }
    }
}

impl IrqScheme for Apic {
    fn is_valid_irq(&self, gsi: usize) -> bool {
        self.ioapic_list.find(gsi as _).is_some()
    }

    fn mask(&self, gsi: usize) -> DeviceResult {
        self.with_ioapic(gsi as _, |apic| {
            apic.toggle(gsi as _, false);
            Ok(())
        })
    }

    fn unmask(&self, gsi: usize) -> DeviceResult {
        self.with_ioapic(gsi as _, |apic| {
            apic.toggle(gsi as _, true);
            Ok(())
        })
    }

    fn configure(&self, gsi: usize, tm: IrqTriggerMode, pol: IrqPolarity) -> DeviceResult {
        let gsi = gsi as u32;
        self.with_ioapic(gsi, |apic| {
            apic.configure(gsi, tm, pol, LocalApic::bsp_id(), 0);
            Ok(())
        })
    }

    fn register_handler(&self, gsi: usize, handler: IrqHandler) -> DeviceResult {
        let gsi = gsi as u32;
        self.with_ioapic(gsi, |apic| {
            let vector = apic.get_vector(gsi) as _; // if not mapped, allocate an available vector by `register_handler()`.
            let vector = self
                .manager_ioapic
                .lock()
                .register_handler(vector, handler)? as u8;
            apic.map_vector(gsi, vector);
            Ok(())
        })
    }

    fn unregister(&self, gsi: usize) -> DeviceResult {
        let gsi = gsi as u32;
        self.with_ioapic(gsi, |apic| {
            let vector = apic.get_vector(gsi) as _;
            self.manager_ioapic.lock().unregister_handler(vector)?;
            apic.map_vector(gsi, 0);
            Ok(())
        })
    }

    fn msi_alloc_block(&self, requested_irqs: usize) -> DeviceResult<Range<usize>> {
        let alloc_size = requested_irqs.next_power_of_two();
        let start = self.manager_ioapic.lock().alloc_block(alloc_size)?;
        Ok(start..start + alloc_size)
    }

    fn msi_free_block(&self, block: Range<usize>) -> DeviceResult {
        self.manager_lapic
            .lock()
            .free_block(block.start, block.len())
    }

    fn msi_register_handler(
        &self,
        block: Range<usize>,
        msi_id: usize,
        handler: IrqHandler,
    ) -> DeviceResult {
        if msi_id < block.len() {
            self.manager_ioapic
                .lock()
                .overwrite_handler(block.start + msi_id, handler)
        } else {
            Err(DeviceError::InvalidParam)
        }
    }
}
