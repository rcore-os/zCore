use alloc::vec::Vec;
use core::{fmt, ptr::NonNull};

use acpi::platform::interrupt::InterruptModel;
use acpi::{AcpiHandler, AcpiTables, PhysicalMapping};
use spin::Mutex;
use x2apic::ioapic::{IoApic as IoApicInner, IrqFlags, IrqMode};

use super::{IrqPolarity, IrqTriggerMode, Phys2VirtFn};

const PAGE_SIZE: usize = 4096;

#[derive(Clone)]
struct AcpiMapHandler {
    phys_to_virt: Phys2VirtFn,
}

impl AcpiHandler for AcpiMapHandler {
    /// we just impl this function, rdsp crate will use it, and we not.
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        // address maybe not aligned, so we align it mannualy
        let aligned_start = physical_address & !(PAGE_SIZE - 1);
        let aligned_end = (physical_address + size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        PhysicalMapping::new(
            physical_address,
            NonNull::new_unchecked((self.phys_to_virt)(physical_address) as *mut T),
            size,
            aligned_end - aligned_start,
            self.clone(),
        )
    }

    /// we do nothing here
    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}
}

/// for apic, we can learn something from here: https://wiki.osdev.org/APIC
pub struct IoApic {
    // ioapic id
    id: u8,
    // gsi means Global System Interrupt, gsi_start is the base number of gsi in this ioapic
    gsi_start: u32,
    // max entry num of interrupt redirection table.
    max_entry: u8,
    // use x2apic crate to help us manipulate apic
    inner: Mutex<IoApicInner>,
}

/// one io-subsystem own one IoApic, maybe we have multiple io-subsystems, so store IoApic in a Vec
#[derive(Debug)]
pub struct IoApicList {
    io_apics: Vec<IoApic>,
}

impl IoApic {
    /// create new IoApic
    pub fn new(id: u8, base_vaddr: usize, gsi_start: u32) -> Self {
        let mut inner = unsafe { IoApicInner::new(base_vaddr as u64) };
        let max_entry = unsafe { inner.max_table_entry() };
        unsafe { assert_eq!(id, inner.id()) };
        // close interrupt
        for i in 0..max_entry + 1 {
            unsafe { inner.disable_irq(i) }
        }
        Self {
            id,
            gsi_start,
            max_entry,
            inner: Mutex::new(inner),
        }
    }

    /// set apic entry irq state by gsi.
    pub fn toggle(&self, gsi: u32, enabled: bool) {
        let idx = (gsi - self.gsi_start) as u8;
        unsafe {
            if enabled {
                self.inner.lock().enable_irq(idx);
            } else {
                self.inner.lock().disable_irq(idx);
            }
        }
    }

    /// get interrupt vector by gsi.
    pub fn get_vector(&self, gsi: u32) -> u8 {
        let idx = (gsi - self.gsi_start) as u8;
        unsafe { self.inner.lock().table_entry(idx).vector() }
    }

    /// set interrupt vector by gsi, vector is a notion of CPU, it is the index of handler in IRQ table
    /// the vector is assigned by fn register_handler.
    pub fn map_vector(&self, gsi: u32, vector: u8) {
        let idx = (gsi - self.gsi_start) as u8;
        let mut inner = self.inner.lock();
        unsafe {
            let mut entry = inner.table_entry(idx);
            entry.set_vector(vector);
            inner.set_table_entry(idx, entry);
        }
    }

    pub fn configure(&self, gsi: u32, tm: IrqTriggerMode, pol: IrqPolarity, dest: u8, vector: u8) {
        let idx = (gsi - self.gsi_start) as u8;
        let mut inner = self.inner.lock();
        let mut entry = unsafe { inner.table_entry(idx) };
        entry.set_vector(vector);
        entry.set_mode(IrqMode::Fixed);
        entry.set_dest(dest);

        let mut flags = IrqFlags::MASKED; // destination mode: physical
        if matches!(tm, IrqTriggerMode::Edge) {
            flags |= IrqFlags::LEVEL_TRIGGERED;
        }
        if matches!(pol, IrqPolarity::ActiveLow) {
            flags |= IrqFlags::LOW_ACTIVE;
        }
        entry.set_flags(flags);

        unsafe { inner.set_table_entry(idx, entry) };
    }
}

impl IoApicList {
    /// create new IoApicList
    pub fn new(acpi_rsdp: usize, phys_to_virt: Phys2VirtFn) -> Self {
        let handler = AcpiMapHandler { phys_to_virt };
        // read rsdt
        let tables = unsafe { AcpiTables::from_rsdp(handler, acpi_rsdp).unwrap() };
        let io_apics =
            if let InterruptModel::Apic(apic) = tables.platform_info().unwrap().interrupt_model {
                apic.io_apics
                    .iter()
                    .map(|i| {
                        IoApic::new(
                            i.id,
                            phys_to_virt(i.address as usize),
                            i.global_system_interrupt_base,
                        )
                    })
                    .collect()
            } else {
                // only legacy i8259 PIC is present
                Vec::new()
            };
        Self { io_apics }
    }

    /// find IoApic by gsi, each IoApic have a range of gsi.
    pub fn find(&self, gsi: u32) -> Option<&IoApic> {
        self.io_apics
            .iter()
            .find(|i| i.gsi_start <= gsi && gsi <= i.gsi_start + i.max_entry as u32)
    }
}

impl fmt::Debug for IoApic {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        struct RedirTable<'a>(&'a IoApic);

        impl<'a> fmt::Debug for RedirTable<'a> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                let mut inner = self.0.inner.lock();
                let count = self.0.max_entry + 1;
                f.debug_list()
                    .entries((0..count).map(|i| unsafe { inner.table_entry(i) }))
                    .finish()
            }
        }

        let version = unsafe { self.inner.lock().version() };
        f.debug_struct("IoApic")
            .field("id", &self.id)
            .field("version", &version)
            .field("gsi_start", &self.gsi_start)
            .field("max_entry", &self.max_entry)
            .field("redir_table", &RedirTable(self))
            .finish()
    }
}
