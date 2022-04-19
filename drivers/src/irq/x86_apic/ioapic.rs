use alloc::vec::Vec;
use core::{fmt, ptr::NonNull};

use acpi::platform::interrupt::InterruptModel;
use acpi::{AcpiHandler, AcpiTables, PhysicalMapping};
use lock::Mutex;
use x2apic::ioapic::{IoApic as IoApicInner, IrqFlags, IrqMode};

use super::{IrqPolarity, IrqTriggerMode, Phys2VirtFn};

const PAGE_SIZE: usize = 4096;

#[derive(Clone)]
struct AcpiMapHandler {
    phys_to_virt: Phys2VirtFn,
}

impl AcpiHandler for AcpiMapHandler {
    /// we just impl this function, `rdsp` crate will use it, and we not.
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

/// An I/O APIC structure.
///
/// For local APIC and I/O APIC, we can learn something from here: <https://wiki.osdev.org/APIC>.
pub struct IoApic {
    /// I/O APIC id.
    id: u8,
    /// GSI means Global System Interrupt, `gsi_start` is the base number of GSI
    /// in this I/O APIC.
    gsi_start: u32,
    /// Max entry num of the interrupt redirection table.
    max_entry: u8,
    /// Use `x2apic` crate to help us manipulate IOAPIC.
    inner: Mutex<IoApicInner>,
}

/// A list of I/O-APICs for systems have multiple I/O subsystems.
#[derive(Debug)]
pub struct IoApicList {
    io_apics: Vec<IoApic>,
}

impl IoApic {
    /// Create a new [`IoApic`] from fields parsed from the ACPI table, and
    /// initialize it by disabling all interrupts.
    pub fn new(id: u8, base_vaddr: usize, gsi_start: u32) -> Self {
        let mut inner = unsafe { IoApicInner::new(base_vaddr as u64) };
        let max_entry = unsafe { inner.max_table_entry() };
        unsafe { assert_eq!(id, inner.id()) };

        unsafe {
            inner.init(super::X86_INT_BASE as u8);
        }
        for i in 0..max_entry + 1 {
            unsafe {
                // disable all interrupts
                inner.disable_irq(i);

                // Clean the redirection table
                let mut entry = inner.table_entry(i);
                entry.set_vector(0);
                entry.set_dest(0);
                entry.set_mode(IrqMode::Fixed);
                entry.set_flags(IrqFlags::MASKED);
                inner.set_table_entry(i, entry);
            }
        }
        Self {
            id,
            gsi_start,
            max_entry,
            inner: Mutex::new(inner),
        }
    }

    /// Set apic entry IRQ state by `gsi`.
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

    /// Get the IDT vector of the `gsi` from redirection table.
    pub fn get_vector(&self, gsi: u32) -> u8 {
        let idx = (gsi - self.gsi_start) as u8;
        unsafe { self.inner.lock().table_entry(idx).vector() }
    }

    /// Set the IDT vector of the `gsi` in redirection table.
    pub fn map_vector(&self, gsi: u32, vector: u8) {
        let idx = (gsi - self.gsi_start) as u8;
        let mut inner = self.inner.lock();
        unsafe {
            let mut entry = inner.table_entry(idx);
            entry.set_vector(vector);
            inner.set_table_entry(idx, entry);
        }
    }

    /// Set the interrupt triggle mode, polarity and other fields of the `gsi`
    /// in redirection table.
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
    /// Probe all I/O APICs from the ACPI table represented by `acpi_rsdp`.
    pub fn new(acpi_rsdp: usize, phys_to_virt: Phys2VirtFn) -> Self {
        let handler = AcpiMapHandler { phys_to_virt };
        // parse ACPI table by the physical address of the RSDP.
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

    /// Get the corresponding I/O APIC of the `gsi`, each I/O-APIC have a range
    /// of GSI number.
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
